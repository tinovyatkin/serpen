use anyhow::Result;
use indexmap::{IndexMap, IndexSet};
use ruff_python_ast::{
    Comprehension, Expr, ExprAttribute, ExprCall, ExprContext, ExprList, ExprName,
    ExprStringLiteral, Identifier, ModModule, Stmt, StmtAssign, StmtFunctionDef, StmtIf,
    StmtImport, StmtImportFrom, StringLiteralValue,
};
use ruff_text_size::TextRange;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use crate::dependency_graph::ModuleNode;

/// Hybrid static bundler that uses sys.modules and hash-based naming
/// This approach avoids forward reference issues while maintaining Python module semantics
pub struct HybridStaticBundler {
    /// Map from original module name to synthetic module name
    module_registry: IndexMap<String, String>,
    /// Map from synthetic module name to init function name
    init_functions: IndexMap<String, String>,
    /// Collected future imports
    future_imports: IndexSet<String>,
    /// Collected stdlib imports that are safe to hoist
    stdlib_imports: Vec<Stmt>,
    /// Track which modules have been bundled
    bundled_modules: IndexSet<String>,
    /// Entry point path for calculating relative paths
    entry_path: Option<String>,
    /// Module export information (for __all__ handling)
    module_exports: IndexMap<String, Option<Vec<String>>>,
}

impl Default for HybridStaticBundler {
    fn default() -> Self {
        Self::new()
    }
}

impl HybridStaticBundler {
    pub fn new() -> Self {
        Self {
            module_registry: IndexMap::new(),
            init_functions: IndexMap::new(),
            future_imports: IndexSet::new(),
            stdlib_imports: Vec::new(),
            bundled_modules: IndexSet::new(),
            entry_path: None,
            module_exports: IndexMap::new(),
        }
    }

    /// Check if a module AST has side effects (executable code at top level)
    /// Returns true if the module has side effects beyond simple definitions
    pub fn has_side_effects(ast: &ModModule) -> bool {
        for stmt in &ast.body {
            match stmt {
                // These statements are pure definitions, no side effects
                Stmt::FunctionDef(_) | Stmt::ClassDef(_) | Stmt::AnnAssign(_) => continue,

                // Simple variable assignments are generally safe
                Stmt::Assign(assign) => {
                    // Special case: __all__ assignments are metadata, not side effects
                    if Self::is_all_assignment(assign) {
                        continue;
                    }
                    // Check if the assignment has function calls or other complex expressions
                    if Self::expression_has_side_effects(&assign.value) {
                        return true;
                    }
                }

                // Import statements are handled separately by the bundler
                Stmt::Import(_) | Stmt::ImportFrom(_) => continue,

                // Type alias statements are safe
                Stmt::TypeAlias(_) => continue,

                // These are definitely side effects
                Stmt::Expr(_)
                | Stmt::If(_)
                | Stmt::While(_)
                | Stmt::For(_)
                | Stmt::With(_)
                | Stmt::Match(_)
                | Stmt::Raise(_)
                | Stmt::Try(_)
                | Stmt::Assert(_)
                | Stmt::Global(_)
                | Stmt::Nonlocal(_)
                | Stmt::Delete(_) => return true,

                // Any other statement type is considered a side effect
                _ => return true,
            }
        }
        false
    }

    /// Check if an expression has side effects
    fn expression_has_side_effects(expr: &Expr) -> bool {
        match expr {
            // Literals and simple names are safe
            Expr::NumberLiteral(_)
            | Expr::StringLiteral(_)
            | Expr::BytesLiteral(_)
            | Expr::BooleanLiteral(_)
            | Expr::NoneLiteral(_)
            | Expr::EllipsisLiteral(_)
            | Expr::Name(_) => false,

            // List/tuple/dict/set literals are safe if their elements are
            Expr::List(list) => list.elts.iter().any(Self::expression_has_side_effects),
            Expr::Tuple(tuple) => tuple.elts.iter().any(Self::expression_has_side_effects),
            Expr::Dict(dict) => dict.items.iter().any(|item| {
                item.key
                    .as_ref()
                    .is_some_and(Self::expression_has_side_effects)
                    || Self::expression_has_side_effects(&item.value)
            }),
            Expr::Set(set) => set.elts.iter().any(Self::expression_has_side_effects),

            // Binary operations on literals are safe
            Expr::BinOp(binop) => {
                Self::expression_has_side_effects(&binop.left)
                    || Self::expression_has_side_effects(&binop.right)
            }

            // Unary operations are safe if the operand is
            Expr::UnaryOp(unaryop) => Self::expression_has_side_effects(&unaryop.operand),

            // Function calls always have potential side effects
            Expr::Call(_) => true,

            // Attribute access might trigger __getattr__, so it's a side effect
            Expr::Attribute(_) => true,

            // Subscripts might trigger __getitem__, so it's a side effect
            Expr::Subscript(_) => true,

            // Comprehensions need recursive checking of their parts
            Expr::ListComp(comp) => {
                Self::expression_has_side_effects(&comp.elt)
                    || Self::generators_have_side_effects(&comp.generators)
            }
            Expr::SetComp(comp) => {
                Self::expression_has_side_effects(&comp.elt)
                    || Self::generators_have_side_effects(&comp.generators)
            }
            Expr::DictComp(comp) => {
                Self::expression_has_side_effects(&comp.key)
                    || Self::expression_has_side_effects(&comp.value)
                    || Self::generators_have_side_effects(&comp.generators)
            }
            Expr::Generator(comp) => {
                Self::expression_has_side_effects(&comp.elt)
                    || Self::generators_have_side_effects(&comp.generators)
            }

            // Any other expression type is considered to have side effects
            _ => true,
        }
    }

    /// Check if an assignment is to __all__
    fn is_all_assignment(assign: &StmtAssign) -> bool {
        if assign.targets.len() != 1 {
            return false;
        }
        matches!(&assign.targets[0], Expr::Name(name) if name.id.as_str() == "__all__")
    }

    /// Check if comprehension generators have side effects
    fn generators_have_side_effects(generators: &[Comprehension]) -> bool {
        for generator in generators {
            // Check the iterator expression
            if Self::expression_has_side_effects(&generator.iter) {
                return true;
            }
            // Check all condition expressions
            for condition in &generator.ifs {
                if Self::expression_has_side_effects(condition) {
                    return true;
                }
            }
        }
        false
    }

    /// Generate a hash from the module's file path relative to entry point
    fn generate_module_hash(&self, module_path: &Path) -> String {
        let mut hasher = Sha256::new();

        // Use the module path for hashing
        let path_str = module_path.to_string_lossy();
        hasher.update(path_str.as_bytes());

        let hash = hasher.finalize();
        // Take first 6 characters of hex for readability
        format!("{:x}", hash)[..6].to_string()
    }

    /// Generate synthetic module name
    fn get_synthetic_module_name(&self, module_name: &str, module_path: &Path) -> String {
        let hash = self.generate_module_hash(module_path);
        let module_name_escaped = module_name
            .chars()
            .map(|c| if c == '.' { '_' } else { c })
            .collect::<String>();
        format!("__cribo_{}_{}", hash, module_name_escaped)
    }

    /// Bundle multiple modules using the hybrid approach
    pub fn bundle_modules(
        &mut self,
        modules: Vec<(String, ModModule, PathBuf)>,
        sorted_module_nodes: &[&ModuleNode],
        entry_module_name: &str,
    ) -> Result<ModModule> {
        let mut final_body = Vec::new();

        log::debug!("Entry module name: {}", entry_module_name);
        log::debug!(
            "Module names in modules vector: {:?}",
            modules.iter().map(|(name, _, _)| name).collect::<Vec<_>>()
        );

        // Store entry path for relative path calculation
        if let Some(entry_node) = sorted_module_nodes.last() {
            self.entry_path = Some(entry_node.path.to_string_lossy().to_string());
        }

        // Track bundled modules
        for (module_name, _, _) in &modules {
            self.bundled_modules.insert(module_name.clone());
        }

        // Check which modules are imported directly (e.g., import module_name)
        let directly_imported_modules =
            self.find_directly_imported_modules(&modules, entry_module_name);
        log::debug!("Directly imported modules: {:?}", directly_imported_modules);

        // Separate modules into inlinable and non-inlinable
        let mut inlinable_modules = Vec::new();
        let mut wrapper_modules = Vec::new();
        let mut module_exports_map = IndexMap::new();

        for (module_name, ast, module_path) in &modules {
            if module_name != entry_module_name {
                // Extract __all__ exports from the module
                let module_exports = self.extract_all_exports(ast);
                module_exports_map.insert(module_name.clone(), module_exports.clone());

                // Check if module can be inlined
                // A module can only be inlined if:
                // 1. It has no side effects
                // 2. It's never imported directly (only from X import Y style)
                if Self::has_side_effects(ast) || directly_imported_modules.contains(module_name) {
                    if Self::has_side_effects(ast) {
                        log::debug!(
                            "Module '{}' has side effects - using wrapper approach",
                            module_name
                        );
                    } else {
                        log::debug!(
                            "Module '{}' is imported directly - using wrapper approach",
                            module_name
                        );
                    }
                    wrapper_modules.push((module_name.clone(), ast.clone(), module_path.clone()));
                } else {
                    log::debug!(
                        "Module '{}' has no side effects and is not imported directly - can be inlined",
                        module_name
                    );
                    inlinable_modules.push((module_name.clone(), ast.clone(), module_path.clone()));
                }
            }
        }

        // First pass: collect imports from ALL modules (for hoisting)
        for (_module_name, ast, _) in &modules {
            self.collect_imports_from_module(ast);
        }

        // Register wrapper modules
        for (module_name, _ast, module_path) in &wrapper_modules {
            self.module_exports.insert(
                module_name.clone(),
                module_exports_map.get(module_name).cloned().flatten(),
            );

            // Register module with synthetic name
            let synthetic_name = self.get_synthetic_module_name(module_name, module_path);
            self.module_registry
                .insert(module_name.clone(), synthetic_name.clone());

            // Register init function
            let init_func_name = format!("__cribo_init_{}", synthetic_name);
            self.init_functions.insert(synthetic_name, init_func_name);
        }

        // Add imports first
        self.add_hoisted_imports(&mut final_body);

        // Check if we need sys import (for wrapper modules)
        let need_sys_import = !wrapper_modules.is_empty();

        if need_sys_import {
            // Add import infrastructure (sys, types imports)
            final_body.extend(self.generate_import_infrastructure_without_registries());

            // Transform wrapper modules into init functions
            for (module_name, ast, module_path) in wrapper_modules {
                let synthetic_name = self.module_registry[&module_name].clone();
                let init_function = self.transform_module_to_init_function(
                    &module_name,
                    &synthetic_name,
                    &module_path,
                    ast,
                )?;
                final_body.push(init_function);
            }

            // Now add the registries after init functions are defined
            final_body.extend(self.generate_registries_and_hook());

            // Initialize wrapper modules in dependency order
            for module_node in sorted_module_nodes {
                if module_node.name != entry_module_name {
                    if let Some(synthetic_name) = self.module_registry.get(&module_node.name) {
                        let init_call = self.generate_module_init_call(synthetic_name);
                        final_body.push(init_call);
                    }
                }
            }
        }

        // Collect global symbols from the entry module first
        let mut global_symbols = self.collect_global_symbols(&modules, entry_module_name);
        let mut symbol_renames = IndexMap::new();

        // Inline the inlinable modules
        for (module_name, ast, _module_path) in inlinable_modules {
            log::debug!("Inlining module '{}'", module_name);
            let inlined_stmts = self.inline_module(
                &module_name,
                ast,
                &module_exports_map,
                &mut global_symbols,
                &mut symbol_renames,
            )?;
            log::debug!(
                "Inlined {} statements from module '{}'",
                inlined_stmts.len(),
                module_name
            );
            final_body.extend(inlined_stmts);
        }

        // Finally, add entry module code (it's always last in topological order)
        for (module_name, ast, _) in modules {
            if module_name == entry_module_name {
                // Entry module - add its code directly at the end
                for stmt in ast.body {
                    if !self.is_hoisted_import(&stmt) {
                        let rewritten_stmts = self.rewrite_import_in_stmt_multiple_with_context(
                            stmt,
                            &module_name,
                            &symbol_renames,
                        );
                        final_body.extend(rewritten_stmts);
                    }
                }
            }
        }

        Ok(ModModule {
            body: final_body,
            range: TextRange::default(),
        })
    }

    /// Transform a module into an initialization function
    fn transform_module_to_init_function(
        &self,
        module_name: &str,
        synthetic_name: &str,
        module_path: &Path,
        ast: ModModule,
    ) -> Result<Stmt> {
        let init_func_name = &self.init_functions[synthetic_name];
        let mut body = Vec::new();

        // Check if module already exists in sys.modules
        body.push(self.create_module_exists_check(synthetic_name));

        // Create module object (returns multiple statements)
        body.extend(self.create_module_object_stmt(synthetic_name, module_path));

        // Register in sys.modules with both synthetic and original names
        body.push(self.create_sys_modules_registration(synthetic_name));
        body.push(self.create_sys_modules_registration_alias(synthetic_name, module_name));

        // Transform module contents
        for stmt in ast.body {
            match &stmt {
                Stmt::Import(_) | Stmt::ImportFrom(_) => {
                    // Transform any import statements in non-entry modules
                    if !self.is_hoisted_import(&stmt) {
                        // For wrapper modules, we don't have symbol renames
                        let empty_renames = IndexMap::new();
                        let transformed_stmts = self.rewrite_import_in_stmt_multiple_with_context(
                            stmt.clone(),
                            module_name,
                            &empty_renames,
                        );
                        body.extend(transformed_stmts);

                        // Check if any imported symbols should be re-exported as module attributes
                        self.add_imported_symbol_attributes(&stmt, module_name, &mut body);
                    }
                }
                Stmt::ClassDef(class_def) => {
                    // Add class definition
                    body.push(stmt.clone());
                    // Set as module attribute only if it should be exported
                    let symbol_name = class_def.name.to_string();
                    if self.should_export_symbol(&symbol_name, module_name) {
                        body.push(self.create_module_attr_assignment("module", &symbol_name));
                    }
                }
                Stmt::FunctionDef(func_def) => {
                    // Add function definition
                    body.push(stmt.clone());
                    // Set as module attribute only if it should be exported
                    let symbol_name = func_def.name.to_string();
                    if self.should_export_symbol(&symbol_name, module_name) {
                        body.push(self.create_module_attr_assignment("module", &symbol_name));
                    }
                }
                Stmt::Assign(assign) => {
                    // For simple assignments, also set as module attribute if it should be exported
                    body.push(stmt.clone());
                    if let Some(name) = self.extract_simple_assign_target(assign) {
                        if self.should_export_symbol(&name, module_name) {
                            body.push(self.create_module_attr_assignment("module", &name));
                        }
                    }
                }
                _ => {
                    // Other statements execute normally
                    body.push(stmt.clone());
                }
            }
        }

        // Generate __all__ for the bundled module only if the original module had __all__
        if let Some(Some(_)) = self.module_exports.get(module_name) {
            body.push(self.create_all_assignment_for_module(module_name));
        }

        // Return the module object
        body.push(Stmt::Return(ruff_python_ast::StmtReturn {
            value: Some(Box::new(Expr::Name(ExprName {
                id: "module".into(),
                ctx: ExprContext::Load,
                range: TextRange::default(),
            }))),
            range: TextRange::default(),
        }));

        // Create the init function
        Ok(Stmt::FunctionDef(StmtFunctionDef {
            name: Identifier::new(init_func_name, TextRange::default()),
            type_params: None,
            parameters: Box::new(ruff_python_ast::Parameters {
                posonlyargs: vec![],
                args: vec![],
                vararg: None,
                kwonlyargs: vec![],
                kwarg: None,
                range: TextRange::default(),
            }),
            returns: None,
            body,
            decorator_list: vec![],
            is_async: false,
            range: TextRange::default(),
        }))
    }

    /// Generate only the sys and types imports
    fn generate_import_infrastructure_without_registries(&self) -> Vec<Stmt> {
        vec![Stmt::Import(StmtImport {
            names: vec![
                ruff_python_ast::Alias {
                    name: Identifier::new("sys", TextRange::default()),
                    asname: None,
                    range: TextRange::default(),
                },
                ruff_python_ast::Alias {
                    name: Identifier::new("types", TextRange::default()),
                    asname: None,
                    range: TextRange::default(),
                },
            ],
            range: TextRange::default(),
        })]
    }

    /// Generate registries and import hook after init functions are defined
    fn generate_registries_and_hook(&self) -> Vec<Stmt> {
        let mut stmts = Vec::new();

        // Create module registry
        stmts.push(self.create_module_registry());

        // Create init functions registry
        stmts.push(self.create_init_functions_registry());

        // Create and install import hook
        stmts.extend(self.create_import_hook());

        stmts
    }

    /// Create the __cribo_modules registry
    fn create_module_registry(&self) -> Stmt {
        let mut items = Vec::new();

        for (original_name, synthetic_name) in &self.module_registry {
            items.push(ruff_python_ast::DictItem {
                key: Some(self.create_string_literal(original_name)),
                value: self.create_string_literal(synthetic_name),
            });
        }

        Stmt::Assign(StmtAssign {
            targets: vec![Expr::Name(ExprName {
                id: "__cribo_modules".into(),
                ctx: ExprContext::Store,
                range: TextRange::default(),
            })],
            value: Box::new(Expr::Dict(ruff_python_ast::ExprDict {
                items,
                range: TextRange::default(),
            })),
            range: TextRange::default(),
        })
    }

    /// Create the __cribo_init_functions registry
    fn create_init_functions_registry(&self) -> Stmt {
        let mut items = Vec::new();

        for (synthetic_name, init_func_name) in &self.init_functions {
            items.push(ruff_python_ast::DictItem {
                key: Some(self.create_string_literal(synthetic_name)),
                value: Expr::Name(ExprName {
                    id: init_func_name.as_str().into(),
                    ctx: ExprContext::Load,
                    range: TextRange::default(),
                }),
            });
        }

        Stmt::Assign(StmtAssign {
            targets: vec![Expr::Name(ExprName {
                id: "__cribo_init_functions".into(),
                ctx: ExprContext::Store,
                range: TextRange::default(),
            })],
            value: Box::new(Expr::Dict(ruff_python_ast::ExprDict {
                items,
                range: TextRange::default(),
            })),
            range: TextRange::default(),
        })
    }

    /// Create the import hook class and install it
    fn create_import_hook(&self) -> Vec<Stmt> {
        let mut stmts = Vec::new();

        // Define CriboBundledFinder class
        let finder_class = self.create_finder_class();
        stmts.push(finder_class);

        // Install the hook: sys.meta_path.insert(0, CriboBundledFinder(__cribo_modules, __cribo_init_functions))
        let install_stmt = Stmt::Expr(ruff_python_ast::StmtExpr {
            value: Box::new(Expr::Call(ExprCall {
                func: Box::new(Expr::Attribute(ExprAttribute {
                    value: Box::new(Expr::Attribute(ExprAttribute {
                        value: Box::new(Expr::Name(ExprName {
                            id: "sys".into(),
                            ctx: ExprContext::Load,
                            range: TextRange::default(),
                        })),
                        attr: Identifier::new("meta_path", TextRange::default()),
                        ctx: ExprContext::Load,
                        range: TextRange::default(),
                    })),
                    attr: Identifier::new("insert", TextRange::default()),
                    ctx: ExprContext::Load,
                    range: TextRange::default(),
                })),
                arguments: ruff_python_ast::Arguments {
                    args: Box::from([
                        self.create_number_literal(0),
                        Expr::Call(ExprCall {
                            func: Box::new(Expr::Name(ExprName {
                                id: "CriboBundledFinder".into(),
                                ctx: ExprContext::Load,
                                range: TextRange::default(),
                            })),
                            arguments: ruff_python_ast::Arguments {
                                args: Box::from([
                                    Expr::Name(ExprName {
                                        id: "__cribo_modules".into(),
                                        ctx: ExprContext::Load,
                                        range: TextRange::default(),
                                    }),
                                    Expr::Name(ExprName {
                                        id: "__cribo_init_functions".into(),
                                        ctx: ExprContext::Load,
                                        range: TextRange::default(),
                                    }),
                                ]),
                                keywords: Box::from([]),
                                range: TextRange::default(),
                            },
                            range: TextRange::default(),
                        }),
                    ]),
                    keywords: Box::from([]),
                    range: TextRange::default(),
                },
                range: TextRange::default(),
            })),
            range: TextRange::default(),
        });

        stmts.push(install_stmt);
        stmts
    }

    /// Create the CriboBundledFinder class
    fn create_finder_class(&self) -> Stmt {
        use ruff_python_ast::{Parameter, ParameterWithDefault, StmtClassDef, StmtReturn};

        let mut class_body = Vec::new();

        // __init__ method
        let init_params = vec![
            ParameterWithDefault {
                parameter: Parameter {
                    name: Identifier::new("self", TextRange::default()),
                    annotation: None,
                    range: TextRange::default(),
                },
                default: None,
                range: TextRange::default(),
            },
            ParameterWithDefault {
                parameter: Parameter {
                    name: Identifier::new("module_registry", TextRange::default()),
                    annotation: None,
                    range: TextRange::default(),
                },
                default: None,
                range: TextRange::default(),
            },
            ParameterWithDefault {
                parameter: Parameter {
                    name: Identifier::new("init_functions", TextRange::default()),
                    annotation: None,
                    range: TextRange::default(),
                },
                default: None,
                range: TextRange::default(),
            },
        ];

        let init_body = vec![
            // self.module_registry = module_registry
            Stmt::Assign(StmtAssign {
                targets: vec![Expr::Attribute(ExprAttribute {
                    value: Box::new(Expr::Name(ExprName {
                        id: "self".into(),
                        ctx: ExprContext::Load,
                        range: TextRange::default(),
                    })),
                    attr: Identifier::new("module_registry", TextRange::default()),
                    ctx: ExprContext::Store,
                    range: TextRange::default(),
                })],
                value: Box::new(Expr::Name(ExprName {
                    id: "module_registry".into(),
                    ctx: ExprContext::Load,
                    range: TextRange::default(),
                })),
                range: TextRange::default(),
            }),
            // self.init_functions = init_functions
            Stmt::Assign(StmtAssign {
                targets: vec![Expr::Attribute(ExprAttribute {
                    value: Box::new(Expr::Name(ExprName {
                        id: "self".into(),
                        ctx: ExprContext::Load,
                        range: TextRange::default(),
                    })),
                    attr: Identifier::new("init_functions", TextRange::default()),
                    ctx: ExprContext::Store,
                    range: TextRange::default(),
                })],
                value: Box::new(Expr::Name(ExprName {
                    id: "init_functions".into(),
                    ctx: ExprContext::Load,
                    range: TextRange::default(),
                })),
                range: TextRange::default(),
            }),
        ];

        let init_method = Stmt::FunctionDef(StmtFunctionDef {
            name: Identifier::new("__init__", TextRange::default()),
            type_params: None,
            parameters: Box::new(ruff_python_ast::Parameters {
                posonlyargs: vec![],
                args: init_params,
                vararg: None,
                kwonlyargs: vec![],
                kwarg: None,
                range: TextRange::default(),
            }),
            returns: None,
            body: init_body,
            decorator_list: vec![],
            is_async: false,
            range: TextRange::default(),
        });

        class_body.push(init_method);

        // find_spec method
        let find_spec_params = vec![
            ParameterWithDefault {
                parameter: Parameter {
                    name: Identifier::new("self", TextRange::default()),
                    annotation: None,
                    range: TextRange::default(),
                },
                default: None,
                range: TextRange::default(),
            },
            ParameterWithDefault {
                parameter: Parameter {
                    name: Identifier::new("fullname", TextRange::default()),
                    annotation: None,
                    range: TextRange::default(),
                },
                default: None,
                range: TextRange::default(),
            },
            ParameterWithDefault {
                parameter: Parameter {
                    name: Identifier::new("path", TextRange::default()),
                    annotation: None,
                    range: TextRange::default(),
                },
                default: None,
                range: TextRange::default(),
            },
            ParameterWithDefault {
                parameter: Parameter {
                    name: Identifier::new("target", TextRange::default()),
                    annotation: None,
                    range: TextRange::default(),
                },
                default: Some(Box::new(Expr::NoneLiteral(
                    ruff_python_ast::ExprNoneLiteral {
                        range: TextRange::default(),
                    },
                ))),
                range: TextRange::default(),
            },
        ];

        let find_spec_kwonlyargs = vec![];

        let mut find_spec_body = Vec::new();

        // if fullname in self.module_registry:
        let condition = Expr::Compare(ruff_python_ast::ExprCompare {
            left: Box::new(Expr::Name(ExprName {
                id: "fullname".into(),
                ctx: ExprContext::Load,
                range: TextRange::default(),
            })),
            ops: Box::from([ruff_python_ast::CmpOp::In]),
            comparators: Box::from([Expr::Attribute(ExprAttribute {
                value: Box::new(Expr::Name(ExprName {
                    id: "self".into(),
                    ctx: ExprContext::Load,
                    range: TextRange::default(),
                })),
                attr: Identifier::new("module_registry", TextRange::default()),
                ctx: ExprContext::Load,
                range: TextRange::default(),
            })]),
            range: TextRange::default(),
        });

        let mut if_body = Vec::new();

        // synthetic_name = self.module_registry[fullname]
        if_body.push(Stmt::Assign(StmtAssign {
            targets: vec![Expr::Name(ExprName {
                id: "synthetic_name".into(),
                ctx: ExprContext::Store,
                range: TextRange::default(),
            })],
            value: Box::new(Expr::Subscript(ruff_python_ast::ExprSubscript {
                value: Box::new(Expr::Attribute(ExprAttribute {
                    value: Box::new(Expr::Name(ExprName {
                        id: "self".into(),
                        ctx: ExprContext::Load,
                        range: TextRange::default(),
                    })),
                    attr: Identifier::new("module_registry", TextRange::default()),
                    ctx: ExprContext::Load,
                    range: TextRange::default(),
                })),
                slice: Box::new(Expr::Name(ExprName {
                    id: "fullname".into(),
                    ctx: ExprContext::Load,
                    range: TextRange::default(),
                })),
                ctx: ExprContext::Load,
                range: TextRange::default(),
            })),
            range: TextRange::default(),
        }));

        // if synthetic_name not in sys.modules:
        let inner_condition = Expr::Compare(ruff_python_ast::ExprCompare {
            left: Box::new(Expr::Name(ExprName {
                id: "synthetic_name".into(),
                ctx: ExprContext::Load,
                range: TextRange::default(),
            })),
            ops: Box::from([ruff_python_ast::CmpOp::NotIn]),
            comparators: Box::from([Expr::Attribute(ExprAttribute {
                value: Box::new(Expr::Name(ExprName {
                    id: "sys".into(),
                    ctx: ExprContext::Load,
                    range: TextRange::default(),
                })),
                attr: Identifier::new("modules", TextRange::default()),
                ctx: ExprContext::Load,
                range: TextRange::default(),
            })]),
            range: TextRange::default(),
        });

        let mut inner_if_body = Vec::new();

        // init_func = self.init_functions.get(synthetic_name)
        inner_if_body.push(Stmt::Assign(StmtAssign {
            targets: vec![Expr::Name(ExprName {
                id: "init_func".into(),
                ctx: ExprContext::Store,
                range: TextRange::default(),
            })],
            value: Box::new(Expr::Call(ExprCall {
                func: Box::new(Expr::Attribute(ExprAttribute {
                    value: Box::new(Expr::Attribute(ExprAttribute {
                        value: Box::new(Expr::Name(ExprName {
                            id: "self".into(),
                            ctx: ExprContext::Load,
                            range: TextRange::default(),
                        })),
                        attr: Identifier::new("init_functions", TextRange::default()),
                        ctx: ExprContext::Load,
                        range: TextRange::default(),
                    })),
                    attr: Identifier::new("get", TextRange::default()),
                    ctx: ExprContext::Load,
                    range: TextRange::default(),
                })),
                arguments: ruff_python_ast::Arguments {
                    args: Box::from([Expr::Name(ExprName {
                        id: "synthetic_name".into(),
                        ctx: ExprContext::Load,
                        range: TextRange::default(),
                    })]),
                    keywords: Box::from([]),
                    range: TextRange::default(),
                },
                range: TextRange::default(),
            })),
            range: TextRange::default(),
        }));

        // if init_func:
        let init_func_condition = Expr::Name(ExprName {
            id: "init_func".into(),
            ctx: ExprContext::Load,
            range: TextRange::default(),
        });

        let init_func_if_body = vec![
            // init_func()
            Stmt::Expr(ruff_python_ast::StmtExpr {
                value: Box::new(Expr::Call(ExprCall {
                    func: Box::new(Expr::Name(ExprName {
                        id: "init_func".into(),
                        ctx: ExprContext::Load,
                        range: TextRange::default(),
                    })),
                    arguments: ruff_python_ast::Arguments {
                        args: Box::from([]),
                        keywords: Box::from([]),
                        range: TextRange::default(),
                    },
                    range: TextRange::default(),
                })),
                range: TextRange::default(),
            }),
        ];

        inner_if_body.push(Stmt::If(StmtIf {
            test: Box::new(init_func_condition),
            body: init_func_if_body,
            elif_else_clauses: vec![],
            range: TextRange::default(),
        }));

        if_body.push(Stmt::If(StmtIf {
            test: Box::new(inner_condition),
            body: inner_if_body,
            elif_else_clauses: vec![],
            range: TextRange::default(),
        }));

        // import importlib.util
        if_body.push(Stmt::Import(StmtImport {
            names: vec![ruff_python_ast::Alias {
                name: Identifier::new("importlib.util", TextRange::default()),
                asname: None,
                range: TextRange::default(),
            }],
            range: TextRange::default(),
        }));

        // return importlib.util.find_spec(synthetic_name)
        if_body.push(Stmt::Return(StmtReturn {
            value: Some(Box::new(Expr::Call(ExprCall {
                func: Box::new(Expr::Attribute(ExprAttribute {
                    value: Box::new(Expr::Attribute(ExprAttribute {
                        value: Box::new(Expr::Name(ExprName {
                            id: "importlib".into(),
                            ctx: ExprContext::Load,
                            range: TextRange::default(),
                        })),
                        attr: Identifier::new("util", TextRange::default()),
                        ctx: ExprContext::Load,
                        range: TextRange::default(),
                    })),
                    attr: Identifier::new("find_spec", TextRange::default()),
                    ctx: ExprContext::Load,
                    range: TextRange::default(),
                })),
                arguments: ruff_python_ast::Arguments {
                    args: Box::from([Expr::Name(ExprName {
                        id: "synthetic_name".into(),
                        ctx: ExprContext::Load,
                        range: TextRange::default(),
                    })]),
                    keywords: Box::from([]),
                    range: TextRange::default(),
                },
                range: TextRange::default(),
            }))),
            range: TextRange::default(),
        }));

        find_spec_body.push(Stmt::If(StmtIf {
            test: Box::new(condition),
            body: if_body,
            elif_else_clauses: vec![],
            range: TextRange::default(),
        }));

        // return None
        find_spec_body.push(Stmt::Return(StmtReturn {
            value: Some(Box::new(Expr::NoneLiteral(
                ruff_python_ast::ExprNoneLiteral {
                    range: TextRange::default(),
                },
            ))),
            range: TextRange::default(),
        }));

        let find_spec_method = Stmt::FunctionDef(StmtFunctionDef {
            name: Identifier::new("find_spec", TextRange::default()),
            type_params: None,
            parameters: Box::new(ruff_python_ast::Parameters {
                posonlyargs: vec![],
                args: find_spec_params,
                vararg: None,
                kwonlyargs: find_spec_kwonlyargs,
                kwarg: None,
                range: TextRange::default(),
            }),
            returns: None,
            body: find_spec_body,
            decorator_list: vec![],
            is_async: false,
            range: TextRange::default(),
        });

        class_body.push(find_spec_method);

        // Create the class definition
        Stmt::ClassDef(StmtClassDef {
            name: Identifier::new("CriboBundledFinder", TextRange::default()),
            type_params: None,
            arguments: None,
            body: class_body,
            decorator_list: vec![],
            range: TextRange::default(),
        })
    }

    /// Create a string literal expression
    fn create_string_literal(&self, value: &str) -> Expr {
        Expr::StringLiteral(ExprStringLiteral {
            value: StringLiteralValue::single(ruff_python_ast::StringLiteral {
                value: value.to_string().into(),
                flags: ruff_python_ast::StringLiteralFlags::empty(),
                range: TextRange::default(),
            }),
            range: TextRange::default(),
        })
    }

    /// Create a number literal expression
    fn create_number_literal(&self, value: i32) -> Expr {
        Expr::NumberLiteral(ruff_python_ast::ExprNumberLiteral {
            value: ruff_python_ast::Number::Int(ruff_python_ast::Int::from(value as u32)),
            range: TextRange::default(),
        })
    }

    /// Check if module exists in sys.modules
    fn create_module_exists_check(&self, synthetic_name: &str) -> Stmt {
        let condition = Expr::Compare(ruff_python_ast::ExprCompare {
            left: Box::new(self.create_string_literal(synthetic_name)),
            ops: Box::from([ruff_python_ast::CmpOp::In]),
            comparators: Box::from([Expr::Attribute(ExprAttribute {
                value: Box::new(Expr::Name(ExprName {
                    id: "sys".into(),
                    ctx: ExprContext::Load,
                    range: TextRange::default(),
                })),
                attr: Identifier::new("modules", TextRange::default()),
                ctx: ExprContext::Load,
                range: TextRange::default(),
            })]),
            range: TextRange::default(),
        });

        Stmt::If(StmtIf {
            test: Box::new(condition),
            body: vec![Stmt::Return(ruff_python_ast::StmtReturn {
                value: Some(Box::new(Expr::Subscript(ruff_python_ast::ExprSubscript {
                    value: Box::new(Expr::Attribute(ExprAttribute {
                        value: Box::new(Expr::Name(ExprName {
                            id: "sys".into(),
                            ctx: ExprContext::Load,
                            range: TextRange::default(),
                        })),
                        attr: Identifier::new("modules", TextRange::default()),
                        ctx: ExprContext::Load,
                        range: TextRange::default(),
                    })),
                    slice: Box::new(self.create_string_literal(synthetic_name)),
                    ctx: ExprContext::Load,
                    range: TextRange::default(),
                }))),
                range: TextRange::default(),
            })],
            elif_else_clauses: vec![],
            range: TextRange::default(),
        })
    }

    /// Create module object
    fn create_module_object_stmt(&self, synthetic_name: &str, module_path: &Path) -> Vec<Stmt> {
        let module_call = Expr::Call(ExprCall {
            func: Box::new(Expr::Attribute(ExprAttribute {
                value: Box::new(Expr::Name(ExprName {
                    id: "types".into(),
                    ctx: ExprContext::Load,
                    range: TextRange::default(),
                })),
                attr: Identifier::new("ModuleType", TextRange::default()),
                ctx: ExprContext::Load,
                range: TextRange::default(),
            })),
            arguments: ruff_python_ast::Arguments {
                args: Box::from([self.create_string_literal(synthetic_name)]),
                keywords: Box::from([]),
                range: TextRange::default(),
            },
            range: TextRange::default(),
        });

        vec![
            // module = types.ModuleType(synthetic_name)
            Stmt::Assign(StmtAssign {
                targets: vec![Expr::Name(ExprName {
                    id: "module".into(),
                    ctx: ExprContext::Store,
                    range: TextRange::default(),
                })],
                value: Box::new(module_call),
                range: TextRange::default(),
            }),
            // module.__file__ = 'original/path.py'
            Stmt::Assign(StmtAssign {
                targets: vec![Expr::Attribute(ExprAttribute {
                    value: Box::new(Expr::Name(ExprName {
                        id: "module".into(),
                        ctx: ExprContext::Load,
                        range: TextRange::default(),
                    })),
                    attr: Identifier::new("__file__", TextRange::default()),
                    ctx: ExprContext::Store,
                    range: TextRange::default(),
                })],
                value: Box::new(self.create_string_literal(&module_path.to_string_lossy())),
                range: TextRange::default(),
            }),
        ]
    }

    /// Register module in sys.modules
    fn create_sys_modules_registration(&self, synthetic_name: &str) -> Stmt {
        Stmt::Assign(StmtAssign {
            targets: vec![Expr::Subscript(ruff_python_ast::ExprSubscript {
                value: Box::new(Expr::Attribute(ExprAttribute {
                    value: Box::new(Expr::Name(ExprName {
                        id: "sys".into(),
                        ctx: ExprContext::Load,
                        range: TextRange::default(),
                    })),
                    attr: Identifier::new("modules", TextRange::default()),
                    ctx: ExprContext::Load,
                    range: TextRange::default(),
                })),
                slice: Box::new(self.create_string_literal(synthetic_name)),
                ctx: ExprContext::Store,
                range: TextRange::default(),
            })],
            value: Box::new(Expr::Name(ExprName {
                id: "module".into(),
                ctx: ExprContext::Load,
                range: TextRange::default(),
            })),
            range: TextRange::default(),
        })
    }

    /// Register module in sys.modules with original name as alias
    fn create_sys_modules_registration_alias(
        &self,
        _synthetic_name: &str,
        original_name: &str,
    ) -> Stmt {
        Stmt::Assign(StmtAssign {
            targets: vec![Expr::Subscript(ruff_python_ast::ExprSubscript {
                value: Box::new(Expr::Attribute(ExprAttribute {
                    value: Box::new(Expr::Name(ExprName {
                        id: "sys".into(),
                        ctx: ExprContext::Load,
                        range: TextRange::default(),
                    })),
                    attr: Identifier::new("modules", TextRange::default()),
                    ctx: ExprContext::Load,
                    range: TextRange::default(),
                })),
                slice: Box::new(self.create_string_literal(original_name)),
                ctx: ExprContext::Store,
                range: TextRange::default(),
            })],
            value: Box::new(Expr::Name(ExprName {
                id: "module".into(),
                ctx: ExprContext::Load,
                range: TextRange::default(),
            })),
            range: TextRange::default(),
        })
    }

    /// Create module attribute assignment
    fn create_module_attr_assignment(&self, module_var: &str, attr_name: &str) -> Stmt {
        Stmt::Assign(StmtAssign {
            targets: vec![Expr::Attribute(ExprAttribute {
                value: Box::new(Expr::Name(ExprName {
                    id: module_var.into(),
                    ctx: ExprContext::Load,
                    range: TextRange::default(),
                })),
                attr: Identifier::new(attr_name, TextRange::default()),
                ctx: ExprContext::Store,
                range: TextRange::default(),
            })],
            value: Box::new(Expr::Name(ExprName {
                id: attr_name.into(),
                ctx: ExprContext::Load,
                range: TextRange::default(),
            })),
            range: TextRange::default(),
        })
    }

    /// Extract simple assignment target
    fn extract_simple_assign_target(&self, assign: &StmtAssign) -> Option<String> {
        if assign.targets.len() == 1 {
            if let Expr::Name(name) = &assign.targets[0] {
                return Some(name.id.to_string());
            }
        }
        None
    }

    /// Generate a call to initialize a module
    fn generate_module_init_call(&self, synthetic_name: &str) -> Stmt {
        if let Some(init_func_name) = self.init_functions.get(synthetic_name) {
            Stmt::Expr(ruff_python_ast::StmtExpr {
                value: Box::new(Expr::Call(ExprCall {
                    func: Box::new(Expr::Name(ExprName {
                        id: init_func_name.as_str().into(),
                        ctx: ExprContext::Load,
                        range: TextRange::default(),
                    })),
                    arguments: ruff_python_ast::Arguments {
                        args: Box::from([]),
                        keywords: Box::from([]),
                        range: TextRange::default(),
                    },
                    range: TextRange::default(),
                })),
                range: TextRange::default(),
            })
        } else {
            Stmt::Pass(ruff_python_ast::StmtPass {
                range: TextRange::default(),
            })
        }
    }

    /// Add hoisted imports to the final body
    fn add_hoisted_imports(&self, final_body: &mut Vec<Stmt>) {
        // Future imports first - combine all into a single import statement
        if !self.future_imports.is_empty() {
            // Sort future imports for deterministic output
            let mut sorted_imports: Vec<String> = self.future_imports.iter().cloned().collect();
            sorted_imports.sort();

            let aliases: Vec<ruff_python_ast::Alias> = sorted_imports
                .into_iter()
                .map(|import| ruff_python_ast::Alias {
                    name: Identifier::new(&import, TextRange::default()),
                    asname: None,
                    range: TextRange::default(),
                })
                .collect();

            final_body.push(Stmt::ImportFrom(StmtImportFrom {
                module: Some(Identifier::new("__future__", TextRange::default())),
                names: aliases,
                level: 0,
                range: TextRange::default(),
            }));
        }

        // Then stdlib imports
        for import_stmt in &self.stdlib_imports {
            final_body.push(import_stmt.clone());
        }
    }

    /// Collect imports from a module for hoisting
    fn collect_imports_from_module(&mut self, ast: &ModModule) {
        for stmt in &ast.body {
            match stmt {
                Stmt::ImportFrom(import_from) => {
                    if let Some(ref module) = import_from.module {
                        let module_name = module.as_str();

                        if module_name == "__future__" {
                            for alias in &import_from.names {
                                self.future_imports.insert(alias.name.to_string());
                            }
                        } else if self.is_safe_stdlib_module(module_name) {
                            self.stdlib_imports.push(stmt.clone());
                        }
                    }
                }
                Stmt::Import(import_stmt) => {
                    for alias in &import_stmt.names {
                        if self.is_safe_stdlib_module(alias.name.as_str()) {
                            self.stdlib_imports.push(stmt.clone());
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Extract __all__ exports from a module
    /// Returns Some(vec) if __all__ is defined, None if not defined
    fn extract_all_exports(&self, ast: &ModModule) -> Option<Vec<String>> {
        for stmt in &ast.body {
            if let Stmt::Assign(assign) = stmt {
                // Look for __all__ = [...]
                if assign.targets.len() == 1 {
                    if let Expr::Name(name) = &assign.targets[0] {
                        if name.id.as_str() == "__all__" {
                            return self.extract_string_list_from_expr(&assign.value);
                        }
                    }
                }
            }
        }
        None
    }

    /// Extract a list of strings from an expression (for __all__ parsing)
    fn extract_string_list_from_expr(&self, expr: &Expr) -> Option<Vec<String>> {
        match expr {
            Expr::List(list_expr) => {
                let mut exports = Vec::new();
                for element in &list_expr.elts {
                    if let Expr::StringLiteral(string_lit) = element {
                        let string_value = string_lit.value.to_str();
                        exports.push(string_value.to_string());
                    }
                }
                Some(exports)
            }
            Expr::Tuple(tuple_expr) => {
                let mut exports = Vec::new();
                for element in &tuple_expr.elts {
                    if let Expr::StringLiteral(string_lit) = element {
                        let string_value = string_lit.value.to_str();
                        exports.push(string_value.to_string());
                    }
                }
                Some(exports)
            }
            _ => None, // Other expressions like computed lists are not supported
        }
    }

    /// Determine if a symbol should be exported based on __all__ or default visibility rules
    fn should_export_symbol(&self, symbol_name: &str, module_name: &str) -> bool {
        // Don't export __all__ itself as a module attribute
        if symbol_name == "__all__" {
            return false;
        }

        // Check if the module has explicit __all__ exports
        if let Some(Some(exports)) = self.module_exports.get(module_name) {
            // Module defines __all__, only export symbols listed there
            exports.contains(&symbol_name.to_string())
        } else {
            // No __all__ defined, use default Python visibility rules
            // Export all symbols that don't start with underscore
            !symbol_name.starts_with('_')
        }
    }

    /// Add module attribute assignments for imported symbols that should be re-exported
    fn add_imported_symbol_attributes(&self, stmt: &Stmt, module_name: &str, body: &mut Vec<Stmt>) {
        match stmt {
            Stmt::ImportFrom(import_from) => {
                // For "from module import symbol1, symbol2 as alias"
                for alias in &import_from.names {
                    let _imported_name = alias.name.as_str();
                    let local_name = alias.asname.as_ref().unwrap_or(&alias.name).as_str();

                    // Check if this imported symbol should be exported
                    if self.should_export_symbol(local_name, module_name) {
                        body.push(self.create_module_attr_assignment("module", local_name));
                    }
                }
            }
            Stmt::Import(import_stmt) => {
                // For "import module" or "import module as alias"
                for alias in &import_stmt.names {
                    let local_name = alias.asname.as_ref().unwrap_or(&alias.name).as_str();

                    // Check if this imported module should be exported
                    if self.should_export_symbol(local_name, module_name) {
                        body.push(self.create_module_attr_assignment("module", local_name));
                    }
                }
            }
            _ => {}
        }
    }

    /// Create an __all__ assignment for a bundled module to make exports explicit
    /// This should only be called for modules that originally defined __all__
    fn create_all_assignment_for_module(&self, module_name: &str) -> Stmt {
        let exported_symbols = self
            .module_exports
            .get(module_name)
            .and_then(|opt| opt.as_ref())
            .cloned()
            .unwrap_or_default();

        // Create string literals for each exported symbol
        let elements: Vec<Expr> = exported_symbols
            .iter()
            .map(|symbol| self.create_string_literal(symbol))
            .collect();

        // Create: module.__all__ = [exported_symbols...]
        Stmt::Assign(StmtAssign {
            targets: vec![Expr::Attribute(ExprAttribute {
                value: Box::new(Expr::Name(ExprName {
                    id: "module".into(),
                    ctx: ExprContext::Load,
                    range: TextRange::default(),
                })),
                attr: Identifier::new("__all__", TextRange::default()),
                ctx: ExprContext::Store,
                range: TextRange::default(),
            })],
            value: Box::new(Expr::List(ExprList {
                elts: elements,
                ctx: ExprContext::Load,
                range: TextRange::default(),
            })),
            range: TextRange::default(),
        })
    }

    /// Check if an import has been hoisted
    fn is_hoisted_import(&self, stmt: &Stmt) -> bool {
        match stmt {
            Stmt::ImportFrom(import_from) => {
                if let Some(ref module) = import_from.module {
                    let module_name = module.as_str();
                    // Only consider __future__ imports as hoisted from entry module
                    module_name == "__future__"
                } else {
                    false
                }
            }
            Stmt::Import(_) => {
                // Regular import statements from entry module should never be considered hoisted
                // They need to be preserved in their original location for aliases to work
                false
            }
            _ => false,
        }
    }

    /// Transform a bundled "from module import ..." statement into multiple assignments
    fn transform_bundled_import_from_multiple(
        &self,
        import_from: StmtImportFrom,
        module_name: &str,
    ) -> Vec<Stmt> {
        let mut assignments = Vec::new();

        for alias in &import_from.names {
            let imported_name = alias.name.as_str();
            let target_name = alias.asname.as_ref().unwrap_or(&alias.name);

            // Create: target = sys.modules['module'].imported_name
            assignments.push(Stmt::Assign(StmtAssign {
                targets: vec![Expr::Name(ExprName {
                    id: target_name.as_str().into(),
                    ctx: ExprContext::Store,
                    range: TextRange::default(),
                })],
                value: Box::new(Expr::Attribute(ExprAttribute {
                    value: Box::new(Expr::Subscript(ruff_python_ast::ExprSubscript {
                        value: Box::new(Expr::Attribute(ExprAttribute {
                            value: Box::new(Expr::Name(ExprName {
                                id: "sys".into(),
                                ctx: ExprContext::Load,
                                range: TextRange::default(),
                            })),
                            attr: Identifier::new("modules", TextRange::default()),
                            ctx: ExprContext::Load,
                            range: TextRange::default(),
                        })),
                        slice: Box::new(self.create_string_literal(module_name)),
                        ctx: ExprContext::Load,
                        range: TextRange::default(),
                    })),
                    attr: Identifier::new(imported_name, TextRange::default()),
                    ctx: ExprContext::Load,
                    range: TextRange::default(),
                })),
                range: TextRange::default(),
            }));
        }

        assignments
    }

    /// Rewrite imports in a statement with module context for relative import resolution
    fn rewrite_import_in_stmt_multiple_with_context(
        &self,
        stmt: Stmt,
        current_module: &str,
        symbol_renames: &IndexMap<String, IndexMap<String, String>>,
    ) -> Vec<Stmt> {
        match stmt {
            Stmt::ImportFrom(import_from) => {
                // Resolve relative imports to absolute module names
                let resolved_module_name =
                    self.resolve_relative_import(&import_from, current_module);

                if let Some(module_name) = resolved_module_name {
                    log::debug!(
                        "Checking if resolved module '{}' is in bundled modules: {:?}",
                        module_name,
                        self.bundled_modules
                    );
                    // Check if this is a bundled module
                    if self.bundled_modules.contains(&module_name) {
                        log::debug!("Transforming bundled import from module: {}", module_name);

                        // Check if this module is in the registry (wrapper approach)
                        // or if it was inlined
                        if self.module_registry.contains_key(&module_name) {
                            // Module uses wrapper approach - transform to sys.modules access
                            return self
                                .transform_bundled_import_from_multiple(import_from, &module_name);
                        } else {
                            // Module was inlined - create assignments for imported symbols
                            log::debug!(
                                "Module '{}' was inlined, creating assignments for imported symbols",
                                module_name
                            );
                            return self.create_assignments_for_inlined_imports(
                                import_from,
                                &module_name,
                                symbol_renames,
                            );
                        }
                    } else {
                        log::debug!(
                            "Module '{}' not found in bundled modules, keeping original import",
                            module_name
                        );
                    }
                }
                vec![Stmt::ImportFrom(import_from)]
            }
            Stmt::Import(import_stmt) => {
                // Check each import individually
                let mut result_stmts = Vec::new();
                let mut handled_all = true;

                for alias in &import_stmt.names {
                    let module_name = alias.name.as_str();
                    if self.bundled_modules.contains(module_name) {
                        if self.module_registry.contains_key(module_name) {
                            // Module uses wrapper approach - transform to sys.modules access
                            let target_name = alias.asname.as_ref().unwrap_or(&alias.name);
                            result_stmts.push(Stmt::Assign(StmtAssign {
                                targets: vec![Expr::Name(ExprName {
                                    id: target_name.as_str().into(),
                                    ctx: ExprContext::Store,
                                    range: TextRange::default(),
                                })],
                                value: Box::new(Expr::Subscript(ruff_python_ast::ExprSubscript {
                                    value: Box::new(Expr::Attribute(ExprAttribute {
                                        value: Box::new(Expr::Name(ExprName {
                                            id: "sys".into(),
                                            ctx: ExprContext::Load,
                                            range: TextRange::default(),
                                        })),
                                        attr: Identifier::new("modules", TextRange::default()),
                                        ctx: ExprContext::Load,
                                        range: TextRange::default(),
                                    })),
                                    slice: Box::new(self.create_string_literal(module_name)),
                                    ctx: ExprContext::Load,
                                    range: TextRange::default(),
                                })),
                                range: TextRange::default(),
                            }));
                        } else {
                            // Module was inlined - this is problematic for direct imports
                            // We need to create a mock module object
                            log::warn!(
                                "Direct import of inlined module '{}' detected - this pattern is not fully supported",
                                module_name
                            );
                            // For now, skip it
                        }
                    } else {
                        handled_all = false;
                    }
                }

                if handled_all {
                    result_stmts
                } else {
                    // Keep original import for non-bundled modules
                    vec![Stmt::Import(import_stmt)]
                }
            }
            _ => vec![stmt],
        }
    }

    /// Check if a module is safe to hoist
    fn is_safe_stdlib_module(&self, module_name: &str) -> bool {
        match module_name {
            // Modules that modify global state - DO NOT HOIST
            "antigravity" | "this" | "__hello__" | "__phello__" => false,
            "site" | "sitecustomize" | "usercustomize" => false,
            "readline" | "rlcompleter" => false,
            "turtle" | "tkinter" => false,
            "webbrowser" => false,
            "platform" | "locale" => false,

            _ => {
                let root_module = module_name.split('.').next().unwrap_or(module_name);
                ruff_python_stdlib::sys::is_known_standard_library(10, root_module)
            }
        }
    }

    /// Resolve a relative import to an absolute module name
    fn resolve_relative_import(
        &self,
        import_from: &StmtImportFrom,
        current_module: &str,
    ) -> Option<String> {
        log::debug!(
            "Resolving relative import: level={}, module={:?}, current_module={}",
            import_from.level,
            import_from.module,
            current_module
        );

        if import_from.level > 0 {
            // This is a relative import
            let mut parts: Vec<&str> = current_module.split('.').collect();

            // For level 1 (.module), we stay in the current package
            // For level 2 (..module), we go up one level, etc.
            // So we remove (level - 1) parts
            for _ in 0..(import_from.level - 1) {
                if parts.is_empty() {
                    log::debug!("Invalid relative import - ran out of parent levels");
                    return None; // Invalid relative import
                }
                parts.pop();
            }

            // Add the module name if specified
            if let Some(ref module) = import_from.module {
                parts.push(module.as_str());
            }

            let resolved = parts.join(".");

            // Check for potential circular import
            if resolved == current_module {
                log::warn!(
                    "Potential circular import detected: {} importing itself",
                    current_module
                );
            }

            log::debug!("Resolved relative import to: {}", resolved);
            Some(resolved)
        } else {
            // Not a relative import
            let resolved = import_from.module.as_ref().map(|m| m.as_str().to_string());
            log::debug!("Not a relative import, resolved to: {:?}", resolved);
            resolved
        }
    }

    /// Find which modules are imported directly in all modules
    fn find_directly_imported_modules(
        &self,
        modules: &[(String, ModModule, PathBuf)],
        _entry_module_name: &str,
    ) -> IndexSet<String> {
        let mut directly_imported = IndexSet::new();

        // Check all modules for direct imports
        for (_module_name, ast, _) in modules {
            for stmt in &ast.body {
                if let Stmt::Import(import_stmt) = stmt {
                    for alias in &import_stmt.names {
                        let imported_module = alias.name.as_str();
                        // Check if this is a bundled module
                        if modules.iter().any(|(name, _, _)| name == imported_module) {
                            directly_imported.insert(imported_module.to_string());
                        }
                    }
                }
            }
        }

        directly_imported
    }

    /// Collect all defined symbols in the global scope
    fn collect_global_symbols(
        &self,
        modules: &[(String, ModModule, PathBuf)],
        entry_module_name: &str,
    ) -> IndexSet<String> {
        let mut global_symbols = IndexSet::new();

        // Collect symbols from all modules that will be in the bundle
        for (module_name, ast, _) in modules {
            if module_name == entry_module_name {
                // For entry module, collect all top-level symbols
                for stmt in &ast.body {
                    match stmt {
                        Stmt::FunctionDef(func_def) => {
                            global_symbols.insert(func_def.name.to_string());
                        }
                        Stmt::ClassDef(class_def) => {
                            global_symbols.insert(class_def.name.to_string());
                        }
                        Stmt::Assign(assign) => {
                            if let Some(name) = self.extract_simple_assign_target(assign) {
                                global_symbols.insert(name);
                            }
                        }
                        Stmt::AnnAssign(ann_assign) => {
                            if let Expr::Name(name) = ann_assign.target.as_ref() {
                                global_symbols.insert(name.id.to_string());
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        global_symbols
    }

    /// Generate a unique symbol name to avoid conflicts
    fn generate_unique_name(&self, base_name: &str, existing_symbols: &IndexSet<String>) -> String {
        if !existing_symbols.contains(base_name) {
            return base_name.to_string();
        }

        // Try adding numeric suffixes
        for i in 1..1000 {
            let candidate = format!("{}${}", base_name, i);
            if !existing_symbols.contains(&candidate) {
                return candidate;
            }
        }

        // Fallback with module prefix
        format!("__cribo_renamed_{}", base_name)
    }

    /// Get a unique name for a symbol, using the same pattern as generate_unique_name
    fn get_unique_name(&self, base_name: &str, existing_symbols: &IndexSet<String>) -> String {
        self.generate_unique_name(base_name, existing_symbols)
    }

    /// Inline a module without side effects directly into the bundle
    fn inline_module(
        &mut self,
        module_name: &str,
        ast: ModModule,
        module_exports_map: &IndexMap<String, Option<Vec<String>>>,
        global_symbols: &mut IndexSet<String>,
        symbol_renames: &mut IndexMap<String, IndexMap<String, String>>,
    ) -> Result<Vec<Stmt>> {
        let mut inlined_stmts = Vec::new();
        let mut module_renames = IndexMap::new();

        // Process each statement in the module
        for stmt in ast.body {
            match &stmt {
                Stmt::Import(_) | Stmt::ImportFrom(_) => {
                    // Skip imports - they should be handled separately
                    if !self.is_hoisted_import(&stmt) {
                        log::debug!(
                            "Skipping import in inlined module '{}': {:?}",
                            module_name,
                            stmt
                        );
                    }
                }
                Stmt::FunctionDef(func_def) => {
                    let func_name = func_def.name.to_string();
                    if self.should_inline_symbol(&func_name, module_name, module_exports_map) {
                        let renamed_name = self.get_unique_name(&func_name, global_symbols);
                        if renamed_name != func_name {
                            module_renames.insert(func_name.clone(), renamed_name.clone());
                            log::debug!(
                                "Renaming function '{}' to '{}' in module '{}'",
                                func_name,
                                renamed_name,
                                module_name
                            );
                        }
                        global_symbols.insert(renamed_name.clone());

                        // Clone and rename the function
                        let mut func_def_clone = func_def.clone();
                        func_def_clone.name = Identifier::new(renamed_name, TextRange::default());
                        inlined_stmts.push(Stmt::FunctionDef(func_def_clone));
                    }
                }
                Stmt::ClassDef(class_def) => {
                    let class_name = class_def.name.to_string();
                    if self.should_inline_symbol(&class_name, module_name, module_exports_map) {
                        let renamed_name = self.get_unique_name(&class_name, global_symbols);
                        if renamed_name != class_name {
                            module_renames.insert(class_name.clone(), renamed_name.clone());
                            log::debug!(
                                "Renaming class '{}' to '{}' in module '{}'",
                                class_name,
                                renamed_name,
                                module_name
                            );
                        }
                        global_symbols.insert(renamed_name.clone());

                        // Clone and rename the class
                        let mut class_def_clone = class_def.clone();
                        class_def_clone.name = Identifier::new(renamed_name, TextRange::default());
                        inlined_stmts.push(Stmt::ClassDef(class_def_clone));
                    }
                }
                Stmt::Assign(assign) => {
                    if let Some(name) = self.extract_simple_assign_target(assign) {
                        if self.should_inline_symbol(&name, module_name, module_exports_map) {
                            let renamed_name = self.get_unique_name(&name, global_symbols);
                            if renamed_name != name {
                                module_renames.insert(name.clone(), renamed_name.clone());
                                log::debug!(
                                    "Renaming variable '{}' to '{}' in module '{}'",
                                    name,
                                    renamed_name,
                                    module_name
                                );
                            }
                            global_symbols.insert(renamed_name.clone());

                            // Clone and rename the assignment
                            let mut assign_clone = assign.clone();
                            if let Expr::Name(name_expr) = &mut assign_clone.targets[0] {
                                name_expr.id = renamed_name.into();
                            }
                            inlined_stmts.push(Stmt::Assign(assign_clone));
                        }
                    }
                }
                Stmt::AnnAssign(ann_assign) => {
                    if let Expr::Name(name) = ann_assign.target.as_ref() {
                        let var_name = name.id.to_string();
                        if self.should_inline_symbol(&var_name, module_name, module_exports_map) {
                            let renamed_name = self.get_unique_name(&var_name, global_symbols);
                            if renamed_name != var_name {
                                module_renames.insert(var_name.clone(), renamed_name.clone());
                                log::debug!(
                                    "Renaming annotated variable '{}' to '{}' in module '{}'",
                                    var_name,
                                    renamed_name,
                                    module_name
                                );
                            }
                            global_symbols.insert(renamed_name.clone());

                            // Clone and rename the annotated assignment
                            let mut ann_assign_clone = ann_assign.clone();
                            if let Expr::Name(name_expr) = ann_assign_clone.target.as_mut() {
                                name_expr.id = renamed_name.into();
                            }
                            inlined_stmts.push(Stmt::AnnAssign(ann_assign_clone));
                        }
                    }
                }
                _ => {
                    // Other statements that shouldn't exist in side-effect-free modules
                    log::warn!(
                        "Unexpected statement type in side-effect-free module '{}': {:?}",
                        module_name,
                        stmt
                    );
                }
            }
        }

        // Store the renames for this module
        if !module_renames.is_empty() {
            symbol_renames.insert(module_name.to_string(), module_renames);
        }

        Ok(inlined_stmts)
    }

    /// Check if a symbol should be inlined based on export rules
    fn should_inline_symbol(
        &self,
        symbol_name: &str,
        module_name: &str,
        module_exports_map: &IndexMap<String, Option<Vec<String>>>,
    ) -> bool {
        let exports = module_exports_map.get(module_name).and_then(|e| e.as_ref());

        if let Some(export_list) = exports {
            // Module has explicit __all__, only inline if exported
            export_list.contains(&symbol_name.to_string())
        } else {
            // No __all__, export non-private symbols
            !symbol_name.starts_with('_')
        }
    }

    /// Create assignment statements for symbols imported from an inlined module
    fn create_assignments_for_inlined_imports(
        &self,
        import_from: StmtImportFrom,
        module_name: &str,
        symbol_renames: &IndexMap<String, IndexMap<String, String>>,
    ) -> Vec<Stmt> {
        let mut assignments = Vec::new();

        for alias in &import_from.names {
            let imported_name = alias.name.as_str();
            let local_name = alias.asname.as_ref().unwrap_or(&alias.name);

            // Check if this symbol was renamed during inlining
            let actual_name = if let Some(module_renames) = symbol_renames.get(module_name) {
                module_renames
                    .get(imported_name)
                    .map(|s| s.as_str())
                    .unwrap_or(imported_name)
            } else {
                imported_name
            };

            // Only create assignment if the names are different
            if local_name.as_str() != actual_name {
                log::debug!(
                    "Creating assignment: {} = {} (from inlined module '{}')",
                    local_name,
                    actual_name,
                    module_name
                );

                let assignment = StmtAssign {
                    targets: vec![Expr::Name(ExprName {
                        id: local_name.as_str().into(),
                        ctx: ExprContext::Store,
                        range: TextRange::default(),
                    })],
                    value: Box::new(Expr::Name(ExprName {
                        id: actual_name.into(),
                        ctx: ExprContext::Load,
                        range: TextRange::default(),
                    })),
                    range: TextRange::default(),
                };

                assignments.push(Stmt::Assign(assignment));
            }
        }

        assignments
    }
}
