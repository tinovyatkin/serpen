use anyhow::Result;
use cribo::static_bundler::StaticBundler;
use ruff_python_ast::{ModModule, Stmt};
use ruff_python_parser::parse_module;

/// Helper function to parse Python code into an AST
fn parse_python(source: &str) -> Result<ModModule> {
    Ok(parse_module(source)?.into_syntax())
}

/// Helper function to get the body of a wrapped module
fn get_wrapped_module_body(wrapped_ast: &ModModule) -> &Vec<Stmt> {
    // The wrapped module should have exactly one statement: the wrapper class
    assert_eq!(wrapped_ast.body.len(), 1);

    // Extract the class body
    match &wrapped_ast.body[0] {
        Stmt::ClassDef(class_def) => &class_def.body,
        _ => panic!("Expected a class definition"),
    }
}

#[test]
fn test_module_wrapper_name_generation() {
    let _bundler = StaticBundler::new();
    let python_code = r#"
def hello():
    return "Hello, World!"
"#;

    let ast = parse_python(python_code).unwrap();
    let mut bundler = StaticBundler::new();
    let wrapped = bundler.wrap_module("mymodule", ast).unwrap();

    assert_eq!(wrapped.wrapper_class_name, "__cribo_module_mymodule");
    assert_eq!(wrapped.original_name, "mymodule");
}

#[test]
fn test_nested_module_wrapper_name() {
    let _bundler = StaticBundler::new();
    let python_code = "x = 1";

    let ast = parse_python(python_code).unwrap();
    let mut bundler = StaticBundler::new();
    let wrapped = bundler
        .wrap_module("package.submodule.module", ast)
        .unwrap();

    assert_eq!(
        wrapped.wrapper_class_name,
        "__cribo_module_package_submodule_module"
    );
}

#[test]
fn test_function_to_static_method_transformation() {
    let python_code = r#"
def greet(name):
    return f"Hello, {name}!"

def calculate(x, y):
    return x + y
"#;

    let ast = parse_python(python_code).unwrap();
    let mut bundler = StaticBundler::new();
    let wrapped = bundler.wrap_module("greetings", ast).unwrap();

    let body = get_wrapped_module_body(&wrapped.transformed_ast);

    // Should have 2 static methods
    let static_methods: Vec<_> = body.iter().filter_map(|stmt| {
        match stmt {
            Stmt::FunctionDef(func_def) => {
                // Check if it has @staticmethod decorator
                let has_staticmethod = func_def.decorator_list.iter().any(|dec| {
                    matches!(&dec.expression, ruff_python_ast::Expr::Name(name) if name.id.as_str() == "staticmethod")
                });
                if has_staticmethod {
                    Some(func_def.name.as_str())
                } else {
                    None
                }
            }
            _ => None,
        }
    }).collect();

    assert_eq!(static_methods.len(), 2);
    assert!(static_methods.contains(&"greet"));
    assert!(static_methods.contains(&"calculate"));
}

#[test]
fn test_class_preservation() {
    let python_code = r#"
class User:
    def __init__(self, name):
        self.name = name
    
    def greet(self):
        return f"Hello, I'm {self.name}"

class Admin(User):
    def __init__(self, name, level):
        super().__init__(name)
        self.level = level
"#;

    let ast = parse_python(python_code).unwrap();
    let mut bundler = StaticBundler::new();
    let wrapped = bundler.wrap_module("models", ast).unwrap();

    let body = get_wrapped_module_body(&wrapped.transformed_ast);

    // Should have 2 class definitions preserved
    let classes: Vec<_> = body
        .iter()
        .filter_map(|stmt| match stmt {
            Stmt::ClassDef(class_def) => Some(class_def.name.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(classes.len(), 2);
    assert!(classes.contains(&"User"));
    assert!(classes.contains(&"Admin"));
}

#[test]
fn test_module_variable_storage() {
    let python_code = r#"
VERSION = "1.0.0"
DEBUG = True
MAX_RETRIES = 3
DEFAULT_CONFIG = {"timeout": 30}
"#;

    let ast = parse_python(python_code).unwrap();
    let mut bundler = StaticBundler::new();
    let wrapped = bundler.wrap_module("config", ast).unwrap();

    let body = get_wrapped_module_body(&wrapped.transformed_ast);

    // Should have __cribo_vars assignment
    let has_cribo_vars = body.iter().any(|stmt| {
        match stmt {
            Stmt::Assign(assign) => {
                // Check if assigning to __cribo_vars
                assign.targets.iter().any(|target| {
                    matches!(target, ruff_python_ast::Expr::Name(name) if name.id.as_str() == "__cribo_vars")
                })
            }
            _ => false,
        }
    });

    assert!(has_cribo_vars, "Should have __cribo_vars assignment");
}

#[test]
fn test_complex_assignment_in_init() {
    let python_code = r#"
# Complex assignments that should go to __cribo_init
x, y = 1, 2
[a, b] = [3, 4]
data["key"] = "value"
obj.attr = 42
"#;

    let ast = parse_python(python_code).unwrap();
    let mut bundler = StaticBundler::new();
    let wrapped = bundler.wrap_module("complex", ast).unwrap();

    let body = get_wrapped_module_body(&wrapped.transformed_ast);

    // Should have __cribo_init method
    let has_cribo_init = body.iter().any(|stmt| match stmt {
        Stmt::FunctionDef(func_def) => func_def.name.as_str() == "__cribo_init",
        _ => false,
    });

    assert!(
        has_cribo_init,
        "Should have __cribo_init method for complex assignments"
    );
}

#[test]
fn test_import_statement_filtering() {
    let python_code = r#"
import os
from typing import List, Dict
import numpy as np

def process_data(data: List[float]) -> np.ndarray:
    return np.array(data)
"#;

    let ast = parse_python(python_code).unwrap();
    let mut bundler = StaticBundler::new();
    let wrapped = bundler.wrap_module("processor", ast).unwrap();

    let body = get_wrapped_module_body(&wrapped.transformed_ast);

    // Should not have any import statements in the class body
    let has_imports = body
        .iter()
        .any(|stmt| matches!(stmt, Stmt::Import(_) | Stmt::ImportFrom(_)));

    assert!(!has_imports, "Import statements should be filtered out");

    // Should have the function as a static method
    let has_process_data = body.iter().any(|stmt| match stmt {
        Stmt::FunctionDef(func_def) => func_def.name.as_str() == "process_data",
        _ => false,
    });

    assert!(has_process_data, "Should have process_data function");
}

#[test]
fn test_mixed_module_content() {
    let python_code = r#"
import json

VERSION = "2.0.0"

class Config:
    def __init__(self):
        self.settings = {}
    
    def load(self, path):
        with open(path) as f:
            self.settings = json.load(f)

def get_default_config():
    return Config()

DEBUG = False

if __name__ == "__main__":
    config = get_default_config()
    print(config)
"#;

    let ast = parse_python(python_code).unwrap();
    let mut bundler = StaticBundler::new();
    let wrapped = bundler.wrap_module("app_config", ast).unwrap();

    let body = get_wrapped_module_body(&wrapped.transformed_ast);

    // Check various elements are present
    let mut has_vars = false;
    let mut has_config_class = false;
    let mut has_get_default_func = false;
    let mut has_init = false;

    for stmt in body {
        match stmt {
            Stmt::Assign(assign) => {
                if assign.targets.iter().any(|t| {
                    matches!(t, ruff_python_ast::Expr::Name(n) if n.id.as_str() == "__cribo_vars")
                }) {
                    has_vars = true;
                }
            }
            Stmt::ClassDef(class_def) => {
                if class_def.name.as_str() == "Config" {
                    has_config_class = true;
                }
            }
            Stmt::FunctionDef(func_def) => match func_def.name.as_str() {
                "get_default_config" => has_get_default_func = true,
                "__cribo_init" => has_init = true,
                _ => {}
            },
            _ => {}
        }
    }

    assert!(has_vars, "Should have __cribo_vars");
    assert!(has_config_class, "Should have Config class");
    assert!(
        has_get_default_func,
        "Should have get_default_config function"
    );
    assert!(has_init, "Should have __cribo_init for complex statements");
}

#[test]
fn test_empty_module() {
    let python_code = "";

    let ast = parse_python(python_code).unwrap();
    let mut bundler = StaticBundler::new();
    let wrapped = bundler.wrap_module("empty", ast).unwrap();

    let body = get_wrapped_module_body(&wrapped.transformed_ast);

    // Empty module should still have a pass statement
    assert!(!body.is_empty(), "Empty modules should have content");
}

#[test]
fn test_module_with_only_imports() {
    let python_code = r#"
import sys
import os
from pathlib import Path
from typing import Optional, List
"#;

    let ast = parse_python(python_code).unwrap();
    let mut bundler = StaticBundler::new();
    let wrapped = bundler.wrap_module("imports_only", ast).unwrap();

    let body = get_wrapped_module_body(&wrapped.transformed_ast);

    // Should have at least one statement (could be pass)
    assert!(!body.is_empty(), "Module should have some content");

    // Should not have any imports
    let has_imports = body
        .iter()
        .any(|stmt| matches!(stmt, Stmt::Import(_) | Stmt::ImportFrom(_)));
    assert!(!has_imports, "Should not have import statements");
}

#[test]
fn test_module_facade_generation() {
    let bundler = StaticBundler::new();

    // Create some fake module nodes
    let utils_module = cribo::dependency_graph::ModuleNode {
        name: "utils".to_string(),
        path: std::path::PathBuf::from("utils.py"),
        imports: vec![],
    };
    let models_user_module = cribo::dependency_graph::ModuleNode {
        name: "models.user".to_string(),
        path: std::path::PathBuf::from("models/user.py"),
        imports: vec![],
    };

    let modules = vec![&utils_module, &models_user_module];

    let facades = bundler.generate_module_facade_creation(&modules);

    // Should have import statement
    let has_types_import = facades.iter().any(|stmt| {
        matches!(stmt, Stmt::Import(import) if import.names.iter().any(|alias| alias.name.as_str() == "types"))
    });
    assert!(has_types_import, "Should import types module");

    // Should have statements for creating module objects and copying attributes
    assert!(
        facades.len() > 1,
        "Should have multiple statements for module facades"
    );
}

#[test]
fn test_expression_statements() {
    let python_code = r#"
# Expression statements that should go to __cribo_init
print("Initializing module")
logging.basicConfig(level=logging.INFO)
app.register_blueprint(api_blueprint)
"#;

    let ast = parse_python(python_code).unwrap();
    let mut bundler = StaticBundler::new();
    let wrapped = bundler.wrap_module("init_module", ast).unwrap();

    let body = get_wrapped_module_body(&wrapped.transformed_ast);

    // Should have __cribo_init with the expression statements
    let init_method = body.iter().find_map(|stmt| match stmt {
        Stmt::FunctionDef(func_def) if func_def.name.as_str() == "__cribo_init" => Some(func_def),
        _ => None,
    });

    assert!(init_method.is_some(), "Should have __cribo_init method");
    assert_eq!(
        init_method.unwrap().body.len(),
        3,
        "Should have 3 expression statements in __cribo_init"
    );
}
