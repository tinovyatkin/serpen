use anyhow::Result;
use cow_utils::CowUtils;
use indexmap::{IndexMap, IndexSet};
use log::debug;
use ruff_python_ast::{
    Decorator, Expr, ExprAttribute, ExprCall, ExprContext, ExprDict, ExprName, ExprStringLiteral,
    Identifier, ModModule, Stmt, StmtAssign, StmtClassDef, StmtExpr, StmtFor, StmtFunctionDef,
    StmtIf, StmtImport, StmtImportFrom, StringLiteralValue,
};
use ruff_text_size::TextRange;

use crate::dependency_graph::ModuleNode;

/// Static bundler that transforms Python modules into wrapper classes
/// eliminating the need for runtime exec() calls
pub struct StaticBundler {
    /// Registry of transformed modules
    #[allow(dead_code)]
    module_registry: IndexMap<String, WrappedModule>,
    /// Module export information (for __all__ handling)
    #[allow(dead_code)]
    module_exports: IndexMap<String, Vec<String>>,
    /// The entry module AST (handled specially, not wrapped)
    entry_module_ast: Option<(String, ModModule)>,
}

/// Represents a module that has been transformed into a wrapper class
pub struct WrappedModule {
    /// The generated wrapper class name (e.g., "__cribo_module_models_user")
    pub wrapper_class_name: String,
    /// The original module name (e.g., "models.user")
    pub original_name: String,
    /// The transformed AST with the module as a class
    pub transformed_ast: ModModule,
}

impl Default for StaticBundler {
    fn default() -> Self {
        Self::new()
    }
}

impl StaticBundler {
    pub fn new() -> Self {
        Self {
            module_registry: IndexMap::new(),
            module_exports: IndexMap::new(),
            entry_module_ast: None,
        }
    }

    /// Bundle multiple modules together with static transformation
    pub fn bundle_modules(
        &mut self,
        modules: Vec<(String, ModModule)>,
        sorted_module_nodes: &[&ModuleNode],
    ) -> Result<ModModule> {
        let mut bundle_ast = ModModule {
            body: Vec::new(),
            range: TextRange::default(),
        };

        // Track which modules have been bundled
        let mut bundled_modules = IndexSet::new();

        // Determine the entry module (last one in sorted order)
        let entry_module_name = sorted_module_nodes
            .last()
            .map(|node| node.name.as_str())
            .unwrap_or("");

        // First, transform all modules into wrapper classes
        for (module_name, ast) in modules {
            let is_entry_module = module_name == entry_module_name;

            if is_entry_module {
                // For the entry module, we'll handle it specially later
                self.entry_module_ast = Some((module_name.clone(), ast));
                // Still add to bundled_modules so imports to it can be resolved
                bundled_modules.insert(module_name.clone());
            } else {
                let wrapped = self.wrap_module(&module_name, ast)?;
                bundled_modules.insert(module_name.clone());

                // Add wrapper class to bundle
                bundle_ast.body.extend(wrapped.transformed_ast.body.clone());

                // Store in registry
                self.module_registry.insert(module_name, wrapped);
            }
        }

        // Add module facade creation code (before processing imports to ensure modules exist)
        let facade_creation_statements = self.generate_module_facade_creation(sorted_module_nodes);
        bundle_ast.body.extend(facade_creation_statements);

        // Add module initialization code (attributes, __cribo_vars, __cribo_init)
        // but exclude the entry module from initialization
        let non_entry_modules: Vec<_> = sorted_module_nodes
            .iter()
            .filter(|node| node.name != entry_module_name)
            .copied()
            .collect();
        let initialization_statements = self.generate_module_initialization(&non_entry_modules);
        bundle_ast.body.extend(initialization_statements);

        // Process all statements to rewrite imports
        let mut final_body = Vec::new();
        for stmt in bundle_ast.body {
            if let Some(rewritten) = self.rewrite_imports(&stmt, &bundled_modules) {
                final_body.extend(rewritten);
            } else if !matches!(stmt, Stmt::Import(_) | Stmt::ImportFrom(_)) {
                // Only keep non-import statements when rewrite_imports returns None
                // Import statements that return None should be completely skipped
                final_body.push(stmt);
            }
            // If it's an import statement and rewrite_imports returned None,
            // it means the import should be completely removed (bundled import)
        }

        // Finally, add the entry module's code directly (not wrapped)
        if let Some((entry_name, entry_ast)) = &self.entry_module_ast {
            debug!("Adding entry module '{}' code directly", entry_name);

            // Process the entry module's statements
            for stmt in &entry_ast.body {
                // Rewrite imports in the entry module too
                if let Some(rewritten) = self.rewrite_imports(stmt, &bundled_modules) {
                    final_body.extend(rewritten);
                } else if !matches!(stmt, Stmt::Import(_) | Stmt::ImportFrom(_)) {
                    // Only keep non-import statements when rewrite_imports returns None
                    // Import statements that return None should be completely skipped
                    final_body.push(stmt.clone());
                }
                // If it's an import statement and rewrite_imports returned None,
                // it means the import should be completely removed (bundled import)
            }
        }

        bundle_ast.body = final_body;
        Ok(bundle_ast)
    }

    /// Helper to create a string literal expression
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

    /// Transform a module into a wrapper class
    pub fn wrap_module(&mut self, module_name: &str, ast: ModModule) -> Result<WrappedModule> {
        let wrapper_class_name = self.generate_wrapper_name(module_name);
        let wrapped_ast = self.transform_module_to_class(module_name, ast)?;

        Ok(WrappedModule {
            wrapper_class_name,
            original_name: module_name.to_string(),
            transformed_ast: wrapped_ast,
        })
    }

    /// Generate a wrapper class name from a module name
    /// e.g., "models.user" -> "__cribo_module_models_user"
    fn generate_wrapper_name(&self, module_name: &str) -> String {
        format!("__cribo_module_{}", module_name.cow_replace('.', "_"))
    }

    /// Transform a module AST into a class definition
    fn transform_module_to_class(&self, module_name: &str, ast: ModModule) -> Result<ModModule> {
        let mut class_body = Vec::new();
        let mut module_vars = IndexMap::new();
        let mut module_init_statements = Vec::new();

        debug!("Transforming module {} to class", module_name);

        for stmt in ast.body {
            match stmt {
                // Functions become static methods
                Stmt::FunctionDef(mut func) => {
                    // Add @staticmethod decorator
                    func.decorator_list
                        .push(self.create_staticmethod_decorator());
                    class_body.push(Stmt::FunctionDef(func));
                }

                // Classes remain as nested classes
                Stmt::ClassDef(class_def) => {
                    class_body.push(Stmt::ClassDef(class_def));
                }

                // Simple assignments go to __cribo_vars only if they don't reference other variables
                Stmt::Assign(ref assign) => {
                    self.categorize_assignment(
                        assign,
                        &mut module_vars,
                        &mut module_init_statements,
                        stmt.clone(),
                    );
                }

                // Import statements are skipped (they're hoisted)
                Stmt::Import(_) | Stmt::ImportFrom(_) => {
                    debug!("Skipping import statement in module transformation");
                }

                // Handle other statement types
                Stmt::If(_)
                | Stmt::While(_)
                | Stmt::For(_)
                | Stmt::With(_)
                | Stmt::Try(_)
                | Stmt::Raise(_)
                | Stmt::Assert(_)
                | Stmt::Global(_)
                | Stmt::Nonlocal(_)
                | Stmt::Return(_)
                | Stmt::Break(_)
                | Stmt::Continue(_)
                | Stmt::Pass(_) => {
                    // Control flow and other statements go into __cribo_init
                    module_init_statements.push(stmt);
                }

                // Annotated assignments can be handled like regular assignments
                Stmt::AnnAssign(ref ann_assign) => {
                    // Try to extract simple variable assignment
                    match (
                        &ann_assign.target.as_ref(),
                        ann_assign.simple,
                        &ann_assign.value,
                    ) {
                        (Expr::Name(name), true, Some(value)) => {
                            // Simple annotated assignment with a value
                            module_vars.insert(name.id.to_string(), value.clone());
                        }
                        _ => {
                            // Complex assignment, no value, or non-simple target
                            module_init_statements.push(stmt);
                        }
                    }
                }

                // Augmented assignments always go to init
                Stmt::AugAssign(_) => {
                    module_init_statements.push(stmt);
                }

                // Expression statements go to init
                Stmt::Expr(_) => {
                    module_init_statements.push(stmt);
                }

                // Delete statements go to init
                Stmt::Delete(_) => {
                    module_init_statements.push(stmt);
                }

                // Type alias statements (Python 3.12+) remain at class level
                Stmt::TypeAlias(_) => {
                    class_body.push(stmt);
                }

                // Match statements (Python 3.10+) go to init
                Stmt::Match(_) => {
                    module_init_statements.push(stmt);
                }

                // IPython-specific statements (if any) go to init
                _ => {
                    module_init_statements.push(stmt);
                }
            }
        }

        // Add __cribo_vars as class attribute if we have module variables
        if !module_vars.is_empty() {
            class_body.push(self.create_module_vars_assignment(module_vars));
        }

        // Add __cribo_init method if we have initialization code
        if !module_init_statements.is_empty() {
            class_body.push(self.create_init_method(module_init_statements));
        }

        // Create the wrapper class
        let wrapper_class =
            self.create_class_def(&self.generate_wrapper_name(module_name), class_body);

        Ok(ModModule {
            body: vec![wrapper_class],
            range: TextRange::default(),
        })
    }

    /// Extract simple assignment target (single name)
    fn extract_simple_target(&self, assign: &StmtAssign) -> Option<String> {
        if assign.targets.len() == 1 {
            if let Expr::Name(name) = &assign.targets[0] {
                return Some(name.id.to_string());
            }
        }
        None
    }

    /// Categorize an assignment statement - either store in module_vars or in init statements
    #[allow(clippy::too_many_arguments)]
    fn categorize_assignment(
        &self,
        assign: &StmtAssign,
        module_vars: &mut IndexMap<String, Box<Expr>>,
        module_init_statements: &mut Vec<Stmt>,
        stmt: Stmt,
    ) {
        if let Some(name) = self.extract_simple_target(assign) {
            // Check if the value contains variable references
            if self.contains_variable_reference(&assign.value) {
                // If it references variables, it needs to go in __cribo_init
                module_init_statements.push(stmt);
            } else {
                // Store the value in module_vars
                module_vars.insert(name, assign.value.clone());
            }
        } else {
            // Complex assignments need special handling in __cribo_init
            module_init_statements.push(stmt);
        }
    }

    /// Check if an expression contains variable references
    #[allow(clippy::only_used_in_recursion)]
    fn contains_variable_reference(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Name(_) | Expr::Attribute(_) => true,
            Expr::Call(call) => {
                // Check if function or any arguments contain variables
                self.contains_variable_reference(&call.func)
                    || call
                        .arguments
                        .args
                        .iter()
                        .any(|arg| self.contains_variable_reference(arg))
            }
            Expr::BinOp(binop) => {
                self.contains_variable_reference(&binop.left)
                    || self.contains_variable_reference(&binop.right)
            }
            Expr::UnaryOp(unaryop) => self.contains_variable_reference(&unaryop.operand),
            Expr::List(list) => list
                .elts
                .iter()
                .any(|e| self.contains_variable_reference(e)),
            Expr::Tuple(tuple) => tuple
                .elts
                .iter()
                .any(|e| self.contains_variable_reference(e)),
            Expr::Dict(dict) => dict.items.iter().any(|item| {
                item.key
                    .as_ref()
                    .is_some_and(|k| self.contains_variable_reference(k))
                    || self.contains_variable_reference(&item.value)
            }),
            Expr::Set(set) => set.elts.iter().any(|e| self.contains_variable_reference(e)),
            // Literals don't contain variable references
            Expr::StringLiteral(_)
            | Expr::BytesLiteral(_)
            | Expr::NumberLiteral(_)
            | Expr::BooleanLiteral(_)
            | Expr::NoneLiteral(_)
            | Expr::EllipsisLiteral(_) => false,
            // For other expression types, assume they might contain variables
            _ => true,
        }
    }

    /// Create @staticmethod decorator
    fn create_staticmethod_decorator(&self) -> Decorator {
        Decorator {
            expression: Expr::Name(ExprName {
                id: Identifier::new("staticmethod", TextRange::default()).into(),
                ctx: ExprContext::Load,
                range: TextRange::default(),
            }),
            range: TextRange::default(),
        }
    }

    /// Create __cribo_vars assignment statement
    fn create_module_vars_assignment(&self, vars: IndexMap<String, Box<Expr>>) -> Stmt {
        let mut items = Vec::new();

        for (name, value) in vars {
            // Create key
            let key = Some(self.create_string_literal(&name));

            // Add to dict
            items.push(ruff_python_ast::DictItem { key, value: *value });
        }

        // Create the dict expression
        let dict_expr = Expr::Dict(ExprDict {
            items,
            range: TextRange::default(),
        });

        // Create assignment: __cribo_vars = {...}
        Stmt::Assign(StmtAssign {
            targets: vec![Expr::Name(ExprName {
                id: Identifier::new("__cribo_vars", TextRange::default()).into(),
                ctx: ExprContext::Store,
                range: TextRange::default(),
            })],
            value: Box::new(dict_expr),
            range: TextRange::default(),
        })
    }

    /// Create __cribo_init method for module initialization code
    fn create_init_method(&self, statements: Vec<Stmt>) -> Stmt {
        let func = StmtFunctionDef {
            name: Identifier::new("__cribo_init", TextRange::default()),
            type_params: None,
            parameters: Box::new(ruff_python_ast::Parameters {
                posonlyargs: vec![],
                args: vec![ruff_python_ast::ParameterWithDefault {
                    parameter: ruff_python_ast::Parameter {
                        name: Identifier::new("cls", TextRange::default()),
                        annotation: None,
                        range: TextRange::default(),
                    },
                    default: None,
                    range: TextRange::default(),
                }],
                vararg: None,
                kwonlyargs: vec![],
                kwarg: None,
                range: TextRange::default(),
            }),
            returns: None,
            body: statements,
            decorator_list: vec![self.create_classmethod_decorator()],
            is_async: false,
            range: TextRange::default(),
        };

        Stmt::FunctionDef(func)
    }

    /// Create @classmethod decorator
    fn create_classmethod_decorator(&self) -> Decorator {
        Decorator {
            expression: Expr::Name(ExprName {
                id: Identifier::new("classmethod", TextRange::default()).into(),
                ctx: ExprContext::Load,
                range: TextRange::default(),
            }),
            range: TextRange::default(),
        }
    }

    /// Create a class definition
    fn create_class_def(&self, name: &str, body: Vec<Stmt>) -> Stmt {
        Stmt::ClassDef(StmtClassDef {
            name: Identifier::new(name, TextRange::default()),
            type_params: None,
            arguments: None,
            body: if body.is_empty() {
                vec![Stmt::Expr(StmtExpr {
                    value: Box::new(self.create_string_literal("pass")),
                    range: TextRange::default(),
                })]
            } else {
                body
            },
            decorator_list: vec![],
            range: TextRange::default(),
        })
    }

    /// Generate module facade creation code (just the module objects)
    pub fn generate_module_facade_creation(&self, sorted_modules: &[&ModuleNode]) -> Vec<Stmt> {
        let mut statements = Vec::new();

        // Import types module
        statements.push(Stmt::Import(StmtImport {
            names: vec![ruff_python_ast::Alias {
                name: Identifier::new("types", TextRange::default()),
                asname: None,
                range: TextRange::default(),
            }],
            range: TextRange::default(),
        }));

        // Create module objects for each module
        for module in sorted_modules {
            let module_name = &module.name;
            // Create module hierarchy
            statements.extend(self.create_module_hierarchy(module_name));
        }

        statements
    }

    /// Generate module initialization code (attributes, vars, init)
    pub fn generate_module_initialization(&self, sorted_modules: &[&ModuleNode]) -> Vec<Stmt> {
        let mut statements = Vec::new();

        // Initialize each module
        for module in sorted_modules {
            let module_name = &module.name;
            let wrapper_name = self.generate_wrapper_name(module_name);

            // Copy attributes from wrapper to module
            statements.extend(self.create_attribute_copying(&wrapper_name, module_name));

            // Call __cribo_init if it exists
            statements.extend(self.create_init_call(&wrapper_name, module_name));
        }

        statements
    }

    /// Create module hierarchy (parent modules)
    fn create_module_hierarchy(&self, module_name: &str) -> Vec<Stmt> {
        let mut statements = Vec::new();
        let parts: Vec<&str> = module_name.split('.').collect();

        for i in 1..=parts.len() {
            let partial_name = parts[..i].join(".");
            statements.extend(self.create_module_object(&partial_name, i == 1));
        }

        statements
    }

    /// Create a module object
    fn create_module_object(&self, module_name: &str, is_root: bool) -> Vec<Stmt> {
        let mut statements = Vec::new();

        // types.ModuleType('module.name')
        let module_type_call = Expr::Call(ExprCall {
            func: Box::new(Expr::Attribute(ExprAttribute {
                value: Box::new(Expr::Name(ExprName {
                    id: Identifier::new("types", TextRange::default()).into(),
                    ctx: ExprContext::Load,
                    range: TextRange::default(),
                })),
                attr: Identifier::new("ModuleType", TextRange::default()),
                ctx: ExprContext::Load,
                range: TextRange::default(),
            })),
            arguments: ruff_python_ast::Arguments {
                args: Box::from([self.create_string_literal(module_name)]),
                keywords: Box::from([]),
                range: TextRange::default(),
            },
            range: TextRange::default(),
        });

        if is_root {
            // Simple assignment for root module
            let parts: Vec<&str> = module_name.split('.').collect();
            statements.push(Stmt::Assign(StmtAssign {
                targets: vec![Expr::Name(ExprName {
                    id: Identifier::new(parts[0], TextRange::default()).into(),
                    ctx: ExprContext::Store,
                    range: TextRange::default(),
                })],
                value: Box::new(module_type_call),
                range: TextRange::default(),
            }));
        } else {
            // Attribute assignment for nested modules
            let target_expr = self.create_module_reference(module_name);
            statements.push(Stmt::Assign(StmtAssign {
                targets: vec![target_expr],
                value: Box::new(module_type_call),
                range: TextRange::default(),
            }));
        }

        statements
    }

    /// Create attribute copying loop
    fn create_attribute_copying(&self, wrapper_name: &str, module_name: &str) -> Vec<Stmt> {
        let mut statements = Vec::new();

        // Create the loop variable
        let loop_var = "__attr";

        // Create dir() call
        let dir_call = Expr::Call(ExprCall {
            func: Box::new(Expr::Name(ExprName {
                id: Identifier::new("dir", TextRange::default()).into(),
                ctx: ExprContext::Load,
                range: TextRange::default(),
            })),
            arguments: ruff_python_ast::Arguments {
                args: Box::from([Expr::Name(ExprName {
                    id: Identifier::new(wrapper_name, TextRange::default()).into(),
                    ctx: ExprContext::Load,
                    range: TextRange::default(),
                })]),
                keywords: Box::from([]),
                range: TextRange::default(),
            },
            range: TextRange::default(),
        });

        // Create the if condition: not __attr.startswith('_')
        let condition = Expr::Call(ExprCall {
            func: Box::new(Expr::Attribute(ExprAttribute {
                value: Box::new(Expr::Name(ExprName {
                    id: Identifier::new(loop_var, TextRange::default()).into(),
                    ctx: ExprContext::Load,
                    range: TextRange::default(),
                })),
                attr: Identifier::new("startswith", TextRange::default()),
                ctx: ExprContext::Load,
                range: TextRange::default(),
            })),
            arguments: ruff_python_ast::Arguments {
                args: Box::from([self.create_string_literal("_")]),
                keywords: Box::from([]),
                range: TextRange::default(),
            },
            range: TextRange::default(),
        });

        let not_condition = Expr::UnaryOp(ruff_python_ast::ExprUnaryOp {
            op: ruff_python_ast::UnaryOp::Not,
            operand: Box::new(condition),
            range: TextRange::default(),
        });

        // Create setattr call
        let setattr_call = self.create_setattr_call(module_name, loop_var, wrapper_name);

        // Create if statement
        let if_stmt = Stmt::If(StmtIf {
            test: Box::new(not_condition),
            body: vec![Stmt::Expr(StmtExpr {
                value: Box::new(setattr_call),
                range: TextRange::default(),
            })],
            elif_else_clauses: vec![],
            range: TextRange::default(),
        });

        // Create for loop
        let for_loop = Stmt::For(StmtFor {
            target: Box::new(Expr::Name(ExprName {
                id: Identifier::new(loop_var, TextRange::default()).into(),
                ctx: ExprContext::Store,
                range: TextRange::default(),
            })),
            iter: Box::new(dir_call),
            body: vec![if_stmt],
            orelse: vec![],
            is_async: false,
            range: TextRange::default(),
        });

        statements.push(for_loop);

        // Also copy __cribo_vars if present
        statements.extend(self.create_vars_copy_statement(wrapper_name, module_name));

        statements
    }

    /// Create setattr call
    fn create_setattr_call(&self, module_name: &str, attr_name: &str, wrapper_name: &str) -> Expr {
        let module_ref = self.create_module_reference(module_name);

        // getattr(__cribo_module_X, __attr)
        let getattr_call = Expr::Call(ExprCall {
            func: Box::new(Expr::Name(ExprName {
                id: Identifier::new("getattr", TextRange::default()).into(),
                ctx: ExprContext::Load,
                range: TextRange::default(),
            })),
            arguments: ruff_python_ast::Arguments {
                args: Box::from([
                    Expr::Name(ExprName {
                        id: Identifier::new(wrapper_name, TextRange::default()).into(),
                        ctx: ExprContext::Load,
                        range: TextRange::default(),
                    }),
                    Expr::Name(ExprName {
                        id: Identifier::new(attr_name, TextRange::default()).into(),
                        ctx: ExprContext::Load,
                        range: TextRange::default(),
                    }),
                ]),
                keywords: Box::from([]),
                range: TextRange::default(),
            },
            range: TextRange::default(),
        });

        // setattr(module.x, __attr, getattr(...))
        Expr::Call(ExprCall {
            func: Box::new(Expr::Name(ExprName {
                id: Identifier::new("setattr", TextRange::default()).into(),
                ctx: ExprContext::Load,
                range: TextRange::default(),
            })),
            arguments: ruff_python_ast::Arguments {
                args: Box::from([
                    module_ref,
                    Expr::Name(ExprName {
                        id: Identifier::new(attr_name, TextRange::default()).into(),
                        ctx: ExprContext::Load,
                        range: TextRange::default(),
                    }),
                    getattr_call,
                ]),
                keywords: Box::from([]),
                range: TextRange::default(),
            },
            range: TextRange::default(),
        })
    }

    /// Create module reference expression (e.g., models.user)
    fn create_module_reference(&self, module_name: &str) -> Expr {
        let parts: Vec<&str> = module_name.split('.').collect();

        if parts.len() == 1 {
            // Simple module name
            Expr::Name(ExprName {
                id: Identifier::new(parts[0], TextRange::default()).into(),
                ctx: ExprContext::Load,
                range: TextRange::default(),
            })
        } else {
            // Nested module - build attribute chain
            let mut expr = Expr::Name(ExprName {
                id: Identifier::new(parts[0], TextRange::default()).into(),
                ctx: ExprContext::Load,
                range: TextRange::default(),
            });

            for part in &parts[1..] {
                expr = Expr::Attribute(ExprAttribute {
                    value: Box::new(expr),
                    attr: Identifier::new(*part, TextRange::default()),
                    ctx: ExprContext::Load,
                    range: TextRange::default(),
                });
            }

            expr
        }
    }

    /// Create statements to copy __cribo_vars
    fn create_vars_copy_statement(&self, wrapper_name: &str, module_name: &str) -> Vec<Stmt> {
        let mut statements = Vec::new();

        // Check if wrapper has __cribo_vars using hasattr
        let has_vars_check = Expr::Call(ExprCall {
            func: Box::new(Expr::Name(ExprName {
                id: Identifier::new("hasattr", TextRange::default()).into(),
                ctx: ExprContext::Load,
                range: TextRange::default(),
            })),
            arguments: ruff_python_ast::Arguments {
                args: Box::from([
                    Expr::Name(ExprName {
                        id: Identifier::new(wrapper_name, TextRange::default()).into(),
                        ctx: ExprContext::Load,
                        range: TextRange::default(),
                    }),
                    self.create_string_literal("__cribo_vars"),
                ]),
                keywords: Box::from([]),
                range: TextRange::default(),
            },
            range: TextRange::default(),
        });

        // Get __cribo_vars
        let get_vars = Expr::Attribute(ExprAttribute {
            value: Box::new(Expr::Name(ExprName {
                id: Identifier::new(wrapper_name, TextRange::default()).into(),
                ctx: ExprContext::Load,
                range: TextRange::default(),
            })),
            attr: Identifier::new("__cribo_vars", TextRange::default()),
            ctx: ExprContext::Load,
            range: TextRange::default(),
        });

        // Create loop variable
        let key_var = "__k";
        let value_var = "__v";

        // Create setattr for each var
        let module_ref = self.create_module_reference(module_name);
        let setattr_call = Expr::Call(ExprCall {
            func: Box::new(Expr::Name(ExprName {
                id: Identifier::new("setattr", TextRange::default()).into(),
                ctx: ExprContext::Load,
                range: TextRange::default(),
            })),
            arguments: ruff_python_ast::Arguments {
                args: Box::from([
                    module_ref,
                    Expr::Name(ExprName {
                        id: Identifier::new(key_var, TextRange::default()).into(),
                        ctx: ExprContext::Load,
                        range: TextRange::default(),
                    }),
                    Expr::Name(ExprName {
                        id: Identifier::new(value_var, TextRange::default()).into(),
                        ctx: ExprContext::Load,
                        range: TextRange::default(),
                    }),
                ]),
                keywords: Box::from([]),
                range: TextRange::default(),
            },
            range: TextRange::default(),
        });

        // Create .items() call
        let items_call = Expr::Call(ExprCall {
            func: Box::new(Expr::Attribute(ExprAttribute {
                value: Box::new(get_vars),
                attr: Identifier::new("items", TextRange::default()),
                ctx: ExprContext::Load,
                range: TextRange::default(),
            })),
            arguments: ruff_python_ast::Arguments {
                args: Box::from([]),
                keywords: Box::from([]),
                range: TextRange::default(),
            },
            range: TextRange::default(),
        });

        // Create tuple target for unpacking
        let tuple_target = Expr::Tuple(ruff_python_ast::ExprTuple {
            elts: vec![
                Expr::Name(ExprName {
                    id: Identifier::new(key_var, TextRange::default()).into(),
                    ctx: ExprContext::Store,
                    range: TextRange::default(),
                }),
                Expr::Name(ExprName {
                    id: Identifier::new(value_var, TextRange::default()).into(),
                    ctx: ExprContext::Store,
                    range: TextRange::default(),
                }),
            ],
            ctx: ExprContext::Store,
            parenthesized: false,
            range: TextRange::default(),
        });

        // Create for loop
        let for_loop = Stmt::For(StmtFor {
            target: Box::new(tuple_target),
            iter: Box::new(items_call),
            body: vec![Stmt::Expr(StmtExpr {
                value: Box::new(setattr_call),
                range: TextRange::default(),
            })],
            orelse: vec![],
            is_async: false,
            range: TextRange::default(),
        });

        // Wrap in if statement
        let if_stmt = Stmt::If(StmtIf {
            test: Box::new(has_vars_check),
            body: vec![for_loop],
            elif_else_clauses: vec![],
            range: TextRange::default(),
        });

        statements.push(if_stmt);
        statements
    }

    /// Create __cribo_init call if method exists
    fn create_init_call(&self, wrapper_name: &str, _module_name: &str) -> Vec<Stmt> {
        let mut statements = Vec::new();

        // Check if wrapper has __cribo_init using hasattr
        let has_init_check = Expr::Call(ExprCall {
            func: Box::new(Expr::Name(ExprName {
                id: Identifier::new("hasattr", TextRange::default()).into(),
                ctx: ExprContext::Load,
                range: TextRange::default(),
            })),
            arguments: ruff_python_ast::Arguments {
                args: Box::from([
                    Expr::Name(ExprName {
                        id: Identifier::new(wrapper_name, TextRange::default()).into(),
                        ctx: ExprContext::Load,
                        range: TextRange::default(),
                    }),
                    self.create_string_literal("__cribo_init"),
                ]),
                keywords: Box::from([]),
                range: TextRange::default(),
            },
            range: TextRange::default(),
        });

        // Call __cribo_init() - no arguments needed since it's a classmethod
        let init_call = Expr::Call(ExprCall {
            func: Box::new(Expr::Attribute(ExprAttribute {
                value: Box::new(Expr::Name(ExprName {
                    id: Identifier::new(wrapper_name, TextRange::default()).into(),
                    ctx: ExprContext::Load,
                    range: TextRange::default(),
                })),
                attr: Identifier::new("__cribo_init", TextRange::default()),
                ctx: ExprContext::Load,
                range: TextRange::default(),
            })),
            arguments: ruff_python_ast::Arguments {
                args: Box::from([]),
                keywords: Box::from([]),
                range: TextRange::default(),
            },
            range: TextRange::default(),
        });

        // Create if statement
        let if_stmt = Stmt::If(StmtIf {
            test: Box::new(has_init_check),
            body: vec![Stmt::Expr(StmtExpr {
                value: Box::new(init_call),
                range: TextRange::default(),
            })],
            elif_else_clauses: vec![],
            range: TextRange::default(),
        });

        statements.push(if_stmt);
        statements
    }

    /// Rewrite import statements to use wrapped modules
    pub fn rewrite_imports(
        &self,
        stmt: &Stmt,
        bundled_modules: &IndexSet<String>,
    ) -> Option<Vec<Stmt>> {
        match stmt {
            Stmt::Import(import) => self.rewrite_import_stmt(import, bundled_modules),
            Stmt::ImportFrom(import_from) => {
                self.rewrite_import_from_stmt(import_from, bundled_modules)
            }
            _ => None,
        }
    }

    /// Rewrite a simple import statement
    fn rewrite_import_stmt(
        &self,
        import: &StmtImport,
        bundled_modules: &IndexSet<String>,
    ) -> Option<Vec<Stmt>> {
        let mut rewritten_statements = Vec::new();
        let mut rewritten_names = Vec::new();

        for alias in &import.names {
            let module_name = alias.name.as_str();

            if bundled_modules.contains(module_name) {
                // This module has been bundled - skip the import entirely
                // The module object has already been created by generate_module_facade_creation
                debug!("Skipping bundled module import: {}", module_name);
            } else {
                // Keep non-bundled imports
                rewritten_names.push(alias.clone());
            }
        }

        // If we still have some imports left, keep the import statement
        if !rewritten_names.is_empty() {
            let new_import = Stmt::Import(StmtImport {
                names: rewritten_names,
                range: import.range,
            });
            rewritten_statements.insert(0, new_import);
        }

        if rewritten_statements.is_empty() {
            None
        } else {
            Some(rewritten_statements)
        }
    }

    /// Rewrite a from-import statement
    fn rewrite_import_from_stmt(
        &self,
        import_from: &StmtImportFrom,
        bundled_modules: &IndexSet<String>,
    ) -> Option<Vec<Stmt>> {
        if let Some(module) = &import_from.module {
            let module_name = module.as_str();

            if bundled_modules.contains(module_name) {
                // This module has been bundled - create assignments for each imported name
                let mut statements = Vec::new();

                for alias in &import_from.names {
                    let imported_name = alias.name.as_str();
                    let target_name = alias
                        .asname
                        .as_ref()
                        .map(|id| id.as_str())
                        .unwrap_or(imported_name);

                    // Create getattr to get the attribute from the module
                    let getattr_call = Expr::Call(ExprCall {
                        func: Box::new(Expr::Name(ExprName {
                            id: Identifier::new("getattr", TextRange::default()).into(),
                            ctx: ExprContext::Load,
                            range: TextRange::default(),
                        })),
                        arguments: ruff_python_ast::Arguments {
                            args: Box::from([
                                self.create_module_reference(module_name),
                                self.create_string_literal(imported_name),
                            ]),
                            keywords: Box::from([]),
                            range: TextRange::default(),
                        },
                        range: TextRange::default(),
                    });

                    // Create assignment
                    let assign = Stmt::Assign(StmtAssign {
                        targets: vec![Expr::Name(ExprName {
                            id: Identifier::new(target_name, TextRange::default()).into(),
                            ctx: ExprContext::Store,
                            range: TextRange::default(),
                        })],
                        value: Box::new(getattr_call),
                        range: TextRange::default(),
                    });

                    statements.push(assign);
                }

                return Some(statements);
            }
        }

        // Keep non-bundled imports as-is
        None
    }
}
