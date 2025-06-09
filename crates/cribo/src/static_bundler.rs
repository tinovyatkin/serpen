use anyhow::Result;
use indexmap::IndexMap;
use log::debug;
use ruff_python_ast::{
    Decorator, Expr, ExprAttribute, ExprCall, ExprContext, ExprDict, ExprName, ExprStringLiteral,
    Identifier, ModModule, Stmt, StmtAssign, StmtClassDef, StmtExpr, StmtFor, StmtFunctionDef,
    StmtIf, StmtImport, StringLiteralValue,
};
use ruff_text_size::TextRange;

use crate::dependency_graph::ModuleNode;

/// Static bundler that transforms Python modules into wrapper classes
/// eliminating the need for runtime exec() calls
pub struct StaticBundler {
    /// Registry of transformed modules
    module_registry: IndexMap<String, WrappedModule>,
    /// Module export information (for __all__ handling)
    module_exports: IndexMap<String, Vec<String>>,
}

/// Represents a module that has been transformed into a wrapper class
struct WrappedModule {
    /// The generated wrapper class name (e.g., "__cribo_module_models_user")
    wrapper_class_name: String,
    /// The original module name (e.g., "models.user")
    original_name: String,
    /// The transformed AST with the module as a class
    transformed_ast: ModModule,
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
        }
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
        format!("__cribo_module_{}", module_name.replace('.', "_"))
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

                // Simple assignments go to __cribo_vars
                Stmt::Assign(ref assign) => {
                    if let Some(name) = self.extract_simple_target(assign) {
                        // Store the value in module_vars
                        module_vars.insert(name, assign.value.clone());
                    } else {
                        // Complex assignments need special handling in __cribo_init
                        module_init_statements.push(stmt);
                    }
                }

                // Import statements are skipped (they're hoisted)
                Stmt::Import(_) | Stmt::ImportFrom(_) => {
                    debug!("Skipping import statement in module transformation");
                }

                // Other statements go into __cribo_init
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
                args: vec![],
                vararg: None,
                kwonlyargs: vec![],
                kwarg: None,
                range: TextRange::default(),
            }),
            returns: None,
            body: statements,
            decorator_list: vec![self.create_staticmethod_decorator()],
            is_async: false,
            range: TextRange::default(),
        };

        Stmt::FunctionDef(func)
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

    /// Generate module facade creation code
    pub fn generate_module_facades(&self, sorted_modules: &[&ModuleNode]) -> Vec<Stmt> {
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

        // Create facades for each module
        for module in sorted_modules {
            let module_name = &module.name;
            let wrapper_name = self.generate_wrapper_name(module_name);

            // Create module hierarchy
            statements.extend(self.create_module_hierarchy(module_name));

            // Copy attributes from wrapper to module
            statements.extend(self.create_attribute_copying(&wrapper_name, module_name));
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
}
