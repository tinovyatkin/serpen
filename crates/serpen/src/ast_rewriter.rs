use anyhow::Result;
use rustpython_parser::ast::{self, Expr, ExprContext, Stmt};
use std::collections::HashMap;

/// Information about an import alias that needs to be resolved
#[derive(Debug, Clone)]
pub struct ImportAlias {
    /// The original name being imported (e.g., "process_data")
    pub original_name: String,
    /// The alias name (e.g., "process_a")
    pub alias_name: String,
    /// The module the import comes from (e.g., "module_a")
    pub module_name: String,
    /// Whether this is a "from" import or a direct import
    pub is_from_import: bool,
    /// Whether this was an explicit alias in the original code (e.g., `as alias_name`)
    pub has_explicit_alias: bool,
}

/// Information about name conflicts that need to be resolved
#[derive(Debug, Clone)]
pub struct NameConflict {
    /// The conflicting name
    pub name: String,
    /// Modules that define this name
    pub modules: Vec<String>,
    /// Renamed versions for each module
    pub renamed_versions: HashMap<String, String>,
}

/// AST rewriter for handling import aliases and name conflicts
pub struct AstRewriter {
    /// Map of import aliases that need to be resolved in the entry module
    import_aliases: HashMap<String, ImportAlias>,
    /// Map of name conflicts and their resolutions
    name_conflicts: HashMap<String, NameConflict>,
    /// Map of renamed identifiers per module
    module_renames: HashMap<String, HashMap<String, String>>,
}

impl AstRewriter {
    pub fn new() -> Self {
        Self {
            import_aliases: HashMap::new(),
            name_conflicts: HashMap::new(),
            module_renames: HashMap::new(),
        }
    }

    /// Collect import aliases from the entry module before they are removed
    pub fn collect_import_aliases(&mut self, entry_ast: &ast::ModModule, _entry_module_name: &str) {
        for stmt in &entry_ast.body {
            match stmt {
                Stmt::ImportFrom(import_from) => {
                    if let Some(module) = &import_from.module {
                        for alias in &import_from.names {
                            if let Some(asname) = &alias.asname {
                                // Import with explicit alias: from module import name as alias
                                let import_alias = ImportAlias {
                                    original_name: alias.name.to_string(),
                                    alias_name: asname.to_string(),
                                    module_name: module.to_string(),
                                    is_from_import: true,
                                    has_explicit_alias: true,
                                };
                                self.import_aliases.insert(asname.to_string(), import_alias);
                            } else {
                                // Import without alias: from module import name
                                let import_alias = ImportAlias {
                                    original_name: alias.name.to_string(),
                                    alias_name: alias.name.to_string(), // Same as original name
                                    module_name: module.to_string(),
                                    is_from_import: true,
                                    has_explicit_alias: false,
                                };
                                self.import_aliases
                                    .insert(alias.name.to_string(), import_alias);
                            }
                        }
                    }
                }
                Stmt::Import(import) => {
                    for alias in &import.names {
                        if let Some(asname) = &alias.asname {
                            let import_alias = ImportAlias {
                                original_name: alias.name.to_string(),
                                alias_name: asname.to_string(),
                                module_name: alias.name.to_string(),
                                is_from_import: false,
                                has_explicit_alias: true,
                            };
                            self.import_aliases.insert(asname.to_string(), import_alias);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Analyze modules to detect name conflicts
    pub fn analyze_name_conflicts(&mut self, modules: &[(String, &ast::ModModule)]) {
        let mut name_to_modules: HashMap<String, Vec<String>> = HashMap::new();

        // Collect all top-level function and class definitions
        for (module_name, module_ast) in modules {
            for stmt in &module_ast.body {
                match stmt {
                    Stmt::FunctionDef(func_def) => {
                        name_to_modules
                            .entry(func_def.name.to_string())
                            .or_default()
                            .push(module_name.clone());
                    }
                    Stmt::ClassDef(class_def) => {
                        name_to_modules
                            .entry(class_def.name.to_string())
                            .or_default()
                            .push(module_name.clone());
                    }
                    Stmt::Assign(assign) => {
                        // Handle module-level variable assignments
                        for target in &assign.targets {
                            if let Expr::Name(name) = target {
                                name_to_modules
                                    .entry(name.id.to_string())
                                    .or_default()
                                    .push(module_name.clone());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        // Find conflicts and generate unique names
        for (name, modules) in name_to_modules {
            if modules.len() > 1 {
                let mut renamed_versions = HashMap::new();
                for module in &modules {
                    let renamed = self.generate_unique_name(&name, module);
                    renamed_versions.insert(module.clone(), renamed.clone());

                    // Track renames for this module
                    self.module_renames
                        .entry(module.clone())
                        .or_default()
                        .insert(name.clone(), renamed);
                }

                let conflict = NameConflict {
                    name: name.clone(),
                    modules: modules.clone(),
                    renamed_versions,
                };
                self.name_conflicts.insert(name, conflict);
            }
        }
    }

    /// Generate a unique name for a conflicting identifier
    fn generate_unique_name(&self, original_name: &str, module_name: &str) -> String {
        // Use module name as prefix to ensure uniqueness
        let module_prefix = module_name.replace(".", "_").replace("-", "_");
        format!("__{}_{}", module_prefix, original_name)
    }

    /// Rewrite a module's AST to resolve name conflicts
    pub fn rewrite_module_ast(
        &self,
        module_name: &str,
        module_ast: &mut ast::ModModule,
    ) -> Result<()> {
        if let Some(renames) = self.module_renames.get(module_name) {
            self.apply_renames_to_ast(&mut module_ast.body, renames)?;
        }
        Ok(())
    }

    /// Apply renames to an AST node recursively
    fn apply_renames_to_ast(
        &self,
        statements: &mut Vec<Stmt>,
        renames: &HashMap<String, String>,
    ) -> Result<()> {
        for stmt in statements {
            self.apply_renames_to_stmt(stmt, renames)?;
        }
        Ok(())
    }

    /// Apply renames to a single statement
    fn apply_renames_to_stmt(
        &self,
        stmt: &mut Stmt,
        renames: &HashMap<String, String>,
    ) -> Result<()> {
        match stmt {
            Stmt::FunctionDef(func_def) => {
                // Rename function definition
                if let Some(new_name) = renames.get(&func_def.name.to_string()) {
                    func_def.name = new_name.clone().into();
                }
                // Rename in return type annotation
                if let Some(returns) = &mut func_def.returns {
                    self.apply_renames_to_expr(returns, renames)?;
                }
                // Rename within function body
                self.apply_renames_to_ast(&mut func_def.body, renames)?;
            }
            Stmt::ClassDef(class_def) => {
                // Rename class definition
                if let Some(new_name) = renames.get(&class_def.name.to_string()) {
                    class_def.name = new_name.clone().into();
                }
                // Rename within class body
                self.apply_renames_to_ast(&mut class_def.body, renames)?;
            }
            Stmt::Assign(assign) => {
                // Rename assignment targets
                for target in &mut assign.targets {
                    self.apply_renames_to_expr(target, renames)?;
                }
                // Rename in assignment value
                self.apply_renames_to_expr(&mut assign.value, renames)?;
            }
            Stmt::Expr(expr_stmt) => {
                self.apply_renames_to_expr(&mut expr_stmt.value, renames)?;
            }
            Stmt::Return(return_stmt) => {
                if let Some(value) = &mut return_stmt.value {
                    self.apply_renames_to_expr(value, renames)?;
                }
            }
            Stmt::If(if_stmt) => {
                self.apply_renames_to_expr(&mut if_stmt.test, renames)?;
                self.apply_renames_to_ast(&mut if_stmt.body, renames)?;
                self.apply_renames_to_ast(&mut if_stmt.orelse, renames)?;
            }
            // Add more statement types as needed
            _ => {}
        }
        Ok(())
    }

    /// Apply renames to an expression
    fn apply_renames_to_expr(
        &self,
        expr: &mut Expr,
        renames: &HashMap<String, String>,
    ) -> Result<()> {
        match expr {
            Expr::Name(name) => {
                // Rename variables in both Load and Store contexts
                if let Some(new_name) = renames.get(&name.id.to_string()) {
                    name.id = new_name.clone().into();
                }
            }
            Expr::Call(call) => {
                self.apply_renames_to_expr(&mut call.func, renames)?;
                for arg in &mut call.args {
                    self.apply_renames_to_expr(arg, renames)?;
                }
                for keyword in &mut call.keywords {
                    self.apply_renames_to_expr(&mut keyword.value, renames)?;
                }
            }
            Expr::Attribute(attr) => {
                self.apply_renames_to_expr(&mut attr.value, renames)?;
            }
            Expr::BinOp(binop) => {
                self.apply_renames_to_expr(&mut binop.left, renames)?;
                self.apply_renames_to_expr(&mut binop.right, renames)?;
            }
            Expr::UnaryOp(unary) => {
                self.apply_renames_to_expr(&mut unary.operand, renames)?;
            }
            Expr::Compare(compare) => {
                self.apply_renames_to_expr(&mut compare.left, renames)?;
                for comparator in &mut compare.comparators {
                    self.apply_renames_to_expr(comparator, renames)?;
                }
            }
            Expr::List(list) => {
                for elt in &mut list.elts {
                    self.apply_renames_to_expr(elt, renames)?;
                }
            }
            Expr::Dict(dict) => {
                for key in &mut dict.keys {
                    if let Some(key) = key {
                        self.apply_renames_to_expr(key, renames)?;
                    }
                }
                for value in &mut dict.values {
                    self.apply_renames_to_expr(value, renames)?;
                }
            }
            // Add more expression types as needed
            _ => {}
        }
        Ok(())
    }

    /// Generate alias assignments for the entry module
    pub fn generate_alias_assignments(&self) -> Vec<Stmt> {
        let mut assignments = Vec::new();

        for (alias_name, import_alias) in &self.import_aliases {
            if import_alias.is_from_import {
                // Check if there's a conflict for this imported name
                let has_conflict = self
                    .name_conflicts
                    .contains_key(&import_alias.original_name);

                // Only generate assignment if:
                // 1. There was an explicit alias in the original code, OR
                // 2. There's a name conflict that requires renaming
                if import_alias.has_explicit_alias || has_conflict {
                    let actual_name = if let Some(conflict) =
                        self.name_conflicts.get(&import_alias.original_name)
                    {
                        // Use the renamed version for this module
                        conflict
                            .renamed_versions
                            .get(&import_alias.module_name)
                            .cloned()
                            .unwrap_or_else(|| import_alias.original_name.clone())
                    } else {
                        import_alias.original_name.clone()
                    };

                    let assignment = ast::StmtAssign {
                        targets: vec![Expr::Name(ast::ExprName {
                            id: alias_name.clone().into(),
                            ctx: ExprContext::Store,
                            range: Default::default(),
                        })],
                        value: Box::new(Expr::Name(ast::ExprName {
                            id: actual_name.into(),
                            ctx: ExprContext::Load,
                            range: Default::default(),
                        })),
                        type_comment: None,
                        range: Default::default(),
                    };
                    assignments.push(Stmt::Assign(assignment));
                }
            }
        }

        assignments
    }

    /// Get debug information about conflicts and aliases
    pub fn get_debug_info(&self) -> String {
        let mut info = String::new();

        info.push_str(&format!(
            "Import Aliases: {} found\n",
            self.import_aliases.len()
        ));
        for (alias, import_info) in &self.import_aliases {
            info.push_str(&format!(
                "  {} -> {} from {}\n",
                alias, import_info.original_name, import_info.module_name
            ));
        }

        info.push_str(&format!(
            "\nName Conflicts: {} found\n",
            self.name_conflicts.len()
        ));
        for (name, conflict) in &self.name_conflicts {
            info.push_str(&format!("  {}: {}\n", name, conflict.modules.join(", ")));
            for (module, renamed) in &conflict.renamed_versions {
                info.push_str(&format!("    {} -> {}\n", module, renamed));
            }
        }

        info
    }
}

impl Default for AstRewriter {
    fn default() -> Self {
        Self::new()
    }
}
