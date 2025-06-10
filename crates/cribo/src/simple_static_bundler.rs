use anyhow::Result;
use indexmap::{IndexMap, IndexSet};
use log::debug;
use ruff_python_ast::{Expr, ExprName, Identifier, ModModule, Stmt, StmtImportFrom};
use ruff_text_size::TextRange;

use crate::dependency_graph::ModuleNode;

/// A simpler static bundler that uses renaming instead of wrapper classes
pub struct SimpleStaticBundler {
    /// Map from original symbol name to renamed symbol name
    /// Format: (module_name, symbol_name) -> renamed_symbol
    symbol_renames: IndexMap<(String, String), String>,
    /// Collected future imports
    future_imports: IndexSet<String>,
    /// Collected stdlib imports
    stdlib_imports: Vec<Stmt>,
    /// Track which modules have been bundled
    bundled_modules: IndexSet<String>,
}

impl Default for SimpleStaticBundler {
    fn default() -> Self {
        Self::new()
    }
}

impl SimpleStaticBundler {
    pub fn new() -> Self {
        Self {
            symbol_renames: IndexMap::new(),
            future_imports: IndexSet::new(),
            stdlib_imports: Vec::new(),
            bundled_modules: IndexSet::new(),
        }
    }

    /// Bundle multiple modules together with static transformation
    pub fn bundle_modules(
        &mut self,
        modules: Vec<(String, ModModule)>,
        sorted_module_nodes: &[&ModuleNode],
    ) -> Result<ModModule> {
        let mut final_body = Vec::new();

        // Track which modules have been bundled
        for (module_name, _) in &modules {
            self.bundled_modules.insert(module_name.clone());
        }

        // Determine the entry module (last one in sorted order)
        let entry_module_name = sorted_module_nodes
            .last()
            .map(|node| node.name.as_str())
            .unwrap_or("");

        // First pass: collect all imports and build symbol rename map
        for (module_name, ast) in &modules {
            if module_name != entry_module_name {
                self.collect_imports_from_module(ast);
                self.build_rename_map(module_name, ast);
            }
        }

        // Add hoisted imports first
        self.add_hoisted_imports(&mut final_body);

        // Second pass: transform and add all non-entry modules
        for (module_name, ast) in modules {
            if module_name != entry_module_name {
                debug!("Processing module: {}", module_name);
                let transformed = self.transform_module(&module_name, ast)?;
                final_body.extend(transformed.body);
            } else {
                // Entry module is processed last
                debug!("Processing entry module: {}", module_name);
                let transformed = self.transform_module(&module_name, ast)?;
                final_body.extend(transformed.body);
            }
        }

        Ok(ModModule {
            body: final_body,
            range: TextRange::default(),
        })
    }

    /// Build rename map for symbols in a module
    fn build_rename_map(&mut self, module_name: &str, ast: &ModModule) {
        for stmt in &ast.body {
            match stmt {
                Stmt::ClassDef(class_def) => {
                    let original_name = class_def.name.as_str();
                    let renamed = format!("__{}__{}", module_name.replace('.', "_"), original_name);
                    self.symbol_renames.insert(
                        (module_name.to_string(), original_name.to_string()),
                        renamed,
                    );
                }
                Stmt::FunctionDef(func_def) => {
                    let original_name = func_def.name.as_str();
                    let renamed = format!("__{}__{}", module_name.replace('.', "_"), original_name);
                    self.symbol_renames.insert(
                        (module_name.to_string(), original_name.to_string()),
                        renamed,
                    );
                }
                _ => {}
            }
        }
    }

    /// Transform a module by renaming its symbols and updating references
    fn transform_module(&self, module_name: &str, mut ast: ModModule) -> Result<ModModule> {
        // Transform the AST
        let mut transformed_body = Vec::new();
        for stmt in ast.body.drain(..) {
            if let Some(transformed) = self.transform_statement(module_name, stmt) {
                transformed_body.push(transformed);
            }
        }

        Ok(ModModule {
            body: transformed_body,
            range: TextRange::default(),
        })
    }

    /// Transform a statement, renaming symbols and updating references
    fn transform_statement(&self, module_name: &str, mut stmt: Stmt) -> Option<Stmt> {
        match &mut stmt {
            Stmt::ClassDef(class_def) => {
                // Rename the class if needed
                if let Some(new_name) = self
                    .symbol_renames
                    .get(&(module_name.to_string(), class_def.name.to_string()))
                {
                    class_def.name = Identifier::new(new_name, TextRange::default());
                }
                // Transform the body
                for body_stmt in &mut class_def.body {
                    self.transform_stmt_recursive(body_stmt);
                }
                Some(stmt)
            }
            Stmt::FunctionDef(func_def) => {
                // Rename the function if needed
                if let Some(new_name) = self
                    .symbol_renames
                    .get(&(module_name.to_string(), func_def.name.to_string()))
                {
                    func_def.name = Identifier::new(new_name, TextRange::default());
                }
                // Transform the body
                for body_stmt in &mut func_def.body {
                    self.transform_stmt_recursive(body_stmt);
                }
                Some(stmt)
            }
            Stmt::Import(import_stmt) => {
                // Skip imports of bundled modules
                let all_bundled = import_stmt
                    .names
                    .iter()
                    .all(|alias| self.bundled_modules.contains(alias.name.as_str()));

                if all_bundled { None } else { Some(stmt) }
            }
            Stmt::ImportFrom(import_from) => {
                // Handle imports from bundled modules
                if let Some(ref module) = import_from.module {
                    if self.bundled_modules.contains(module.as_str()) {
                        // Skip this import - symbols are renamed and available directly
                        return None;
                    }
                }
                // Transform any references in the import
                self.transform_stmt_recursive(&mut stmt);
                Some(stmt)
            }
            _ => {
                // Transform any references in other statements
                self.transform_stmt_recursive(&mut stmt);
                Some(stmt)
            }
        }
    }

    /// Recursively transform expressions in a statement
    fn transform_stmt_recursive(&self, stmt: &mut Stmt) {
        match stmt {
            Stmt::Expr(expr_stmt) => {
                self.transform_expr_recursive(&mut expr_stmt.value);
            }
            Stmt::Assign(assign) => {
                for target in &mut assign.targets {
                    self.transform_expr_recursive(target);
                }
                self.transform_expr_recursive(&mut assign.value);
            }
            Stmt::Return(ret) => {
                if let Some(ref mut value) = ret.value {
                    self.transform_expr_recursive(value);
                }
            }
            Stmt::If(if_stmt) => {
                self.transform_expr_recursive(&mut if_stmt.test);
                for body_stmt in &mut if_stmt.body {
                    self.transform_stmt_recursive(body_stmt);
                }
                for elif_else in &mut if_stmt.elif_else_clauses {
                    if let Some(ref mut test) = elif_else.test {
                        self.transform_expr_recursive(test);
                    }
                    for body_stmt in &mut elif_else.body {
                        self.transform_stmt_recursive(body_stmt);
                    }
                }
            }
            Stmt::For(for_stmt) => {
                self.transform_expr_recursive(&mut for_stmt.target);
                self.transform_expr_recursive(&mut for_stmt.iter);
                for body_stmt in &mut for_stmt.body {
                    self.transform_stmt_recursive(body_stmt);
                }
                for else_stmt in &mut for_stmt.orelse {
                    self.transform_stmt_recursive(else_stmt);
                }
            }
            Stmt::While(while_stmt) => {
                self.transform_expr_recursive(&mut while_stmt.test);
                for body_stmt in &mut while_stmt.body {
                    self.transform_stmt_recursive(body_stmt);
                }
                for else_stmt in &mut while_stmt.orelse {
                    self.transform_stmt_recursive(else_stmt);
                }
            }
            // Add more statement types as needed
            _ => {}
        }
    }

    /// Recursively transform an expression
    fn transform_expr_recursive(&self, expr: &mut Expr) {
        match expr {
            // Handle direct name references
            Expr::Name(name_expr) => {
                // Check if this name should be renamed
                for ((mod_name, symbol), renamed) in self.symbol_renames.iter() {
                    if symbol == name_expr.id.as_str() {
                        // Only rename if it's from a bundled module
                        if self.bundled_modules.contains(mod_name) {
                            name_expr.id = Identifier::new(renamed, TextRange::default()).into();
                            return;
                        }
                    }
                }
            }
            // Handle attribute access (e.g., models.User)
            Expr::Attribute(attr_expr) => {
                if let Expr::Name(module_ref) = &attr_expr.value.as_ref() {
                    let module_name = module_ref.id.as_str();
                    let attr_name = attr_expr.attr.as_str();

                    if let Some(renamed) = self.get_renamed_symbol(module_name, attr_name) {
                        // Replace module.attr with renamed symbol
                        *expr = Expr::Name(ExprName {
                            id: Identifier::new(&renamed, TextRange::default()).into(),
                            ctx: attr_expr.ctx,
                            range: TextRange::default(),
                        });
                        return;
                    }
                }
                // Otherwise, continue transforming
                self.transform_expr_recursive(&mut attr_expr.value);
            }
            Expr::Call(call) => {
                self.transform_expr_recursive(&mut call.func);
                for arg in &mut call.arguments.args {
                    self.transform_expr_recursive(arg);
                }
                for keyword in &mut call.arguments.keywords {
                    self.transform_expr_recursive(&mut keyword.value);
                }
            }
            Expr::BinOp(binop) => {
                self.transform_expr_recursive(&mut binop.left);
                self.transform_expr_recursive(&mut binop.right);
            }
            Expr::UnaryOp(unaryop) => {
                self.transform_expr_recursive(&mut unaryop.operand);
            }
            Expr::List(list) => {
                for elt in &mut list.elts {
                    self.transform_expr_recursive(elt);
                }
            }
            Expr::Tuple(tuple) => {
                for elt in &mut tuple.elts {
                    self.transform_expr_recursive(elt);
                }
            }
            Expr::Dict(dict) => {
                for item in &mut dict.items {
                    if let Some(ref mut key) = item.key {
                        self.transform_expr_recursive(key);
                    }
                    self.transform_expr_recursive(&mut item.value);
                }
            }
            // Add more expression types as needed
            _ => {}
        }
    }

    /// Get the renamed symbol for a given reference
    fn get_renamed_symbol(&self, module: &str, symbol: &str) -> Option<String> {
        self.symbol_renames
            .get(&(module.to_string(), symbol.to_string()))
            .cloned()
    }

    /// Collect imports from a module for hoisting
    fn collect_imports_from_module(&mut self, ast: &ModModule) {
        for stmt in &ast.body {
            match stmt {
                Stmt::ImportFrom(import_from) => {
                    if let Some(ref module) = import_from.module {
                        let module_name = module.as_str();

                        // Collect future imports
                        if module_name == "__future__" {
                            for alias in &import_from.names {
                                self.future_imports.insert(alias.name.to_string());
                            }
                        }
                        // Collect safe stdlib imports
                        else if self.is_safe_stdlib_module(module_name) {
                            self.stdlib_imports.push(stmt.clone());
                        }
                    }
                }
                Stmt::Import(import_stmt) => {
                    // Check if this is a safe stdlib import
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

    /// Add hoisted imports to the final body
    fn add_hoisted_imports(&self, final_body: &mut Vec<Stmt>) {
        // Future imports must come first
        for future_import in &self.future_imports {
            let stmt = Stmt::ImportFrom(StmtImportFrom {
                module: Some(Identifier::new("__future__", TextRange::default())),
                names: vec![ruff_python_ast::Alias {
                    name: Identifier::new(future_import, TextRange::default()),
                    asname: None,
                    range: TextRange::default(),
                }],
                level: 0,
                range: TextRange::default(),
            });
            final_body.push(stmt);
        }

        // Then add safe stdlib imports
        for import_stmt in &self.stdlib_imports {
            final_body.push(import_stmt.clone());
        }
    }

    /// Check if a module is a safe stdlib module that can be hoisted
    fn is_safe_stdlib_module(&self, module_name: &str) -> bool {
        // Modules that modify global state or have side effects - DO NOT HOIST
        match module_name {
            "antigravity" | "this" | "__hello__" | "__phello__" => false,
            "site" | "sitecustomize" | "usercustomize" => false,
            "readline" | "rlcompleter" => false, // Terminal state
            "turtle" | "tkinter" => false,       // GUI initialization
            "webbrowser" => false,               // May open browser
            "platform" | "locale" => false,      // System queries that might be order-dependent

            // For all other modules, check if they're stdlib
            _ => {
                let root_module = module_name.split('.').next().unwrap_or(module_name);
                // We'll accept Python 3.10 as baseline for now
                ruff_python_stdlib::sys::is_known_standard_library(10, root_module)
            }
        }
    }
}
