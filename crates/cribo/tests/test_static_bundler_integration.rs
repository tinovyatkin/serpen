use anyhow::Result;
use cribo::dependency_graph::ModuleNode;
use cribo::static_bundler::StaticBundler;
use ruff_python_ast::{ModModule, Stmt};
use ruff_python_codegen::{Generator, Stylist};
use ruff_python_parser::parse_module;
use std::path::PathBuf;

/// Helper function to parse Python code into an AST
fn parse_python(source: &str) -> Result<ModModule> {
    Ok(parse_module(source)?.into_syntax())
}

/// Helper to generate Python code from AST
fn generate_python(module: &ModModule) -> String {
    let empty_parsed = ruff_python_parser::parse_module("").expect("Failed to parse empty module");
    let stylist = Stylist::from_tokens(empty_parsed.tokens(), "");

    let mut code_parts = Vec::new();
    for stmt in &module.body {
        let generator = Generator::from(&stylist);
        let stmt_code = generator.stmt(stmt);
        code_parts.push(stmt_code);
    }

    code_parts.join("\n")
}

#[test]
fn test_simple_module_bundling() {
    // Create a simple two-module system
    let utils_code = r#"
def helper(x):
    return x * 2

VERSION = "1.0.0"
"#;

    let main_code = r#"
import utils

result = utils.helper(21)
print(f"Result: {result}")
print(f"Version: {utils.VERSION}")
"#;

    let utils_ast = parse_python(utils_code).expect("Failed to parse utils");
    let main_ast = parse_python(main_code).expect("Failed to parse main");

    let mut bundler = StaticBundler::new();

    // Create module nodes for dependency order
    let utils_node = ModuleNode {
        name: "utils".to_string(),
        path: PathBuf::from("utils.py"),
        imports: vec![],
    };

    let main_node = ModuleNode {
        name: "main".to_string(),
        path: PathBuf::from("main.py"),
        imports: vec!["utils".to_string()],
    };

    let sorted_nodes = vec![&utils_node, &main_node];

    // Bundle the modules
    let modules = vec![
        ("utils".to_string(), utils_ast),
        ("main".to_string(), main_ast),
    ];

    let bundled_ast = bundler
        .bundle_modules(modules, &sorted_nodes)
        .expect("Failed to bundle modules");

    // Check that we have wrapper classes for non-entry modules
    let has_utils_wrapper = bundled_ast.body.iter().any(|stmt| {
        matches!(stmt, Stmt::ClassDef(class_def) if class_def.name.as_str() == "__cribo_module_utils")
    });

    // The entry module (main) should NOT be wrapped - it executes directly
    let has_main_wrapper = bundled_ast.body.iter().any(|stmt| {
        matches!(stmt, Stmt::ClassDef(class_def) if class_def.name.as_str() == "__cribo_module_main")
    });

    assert!(has_utils_wrapper, "Should have utils wrapper class");
    assert!(
        !has_main_wrapper,
        "Entry module should NOT be wrapped in class"
    );

    // Check that imports have been rewritten
    let generated_code = generate_python(&bundled_ast);

    // Should not contain "import utils" anymore
    assert!(
        !generated_code.contains("import utils"),
        "Import should be rewritten"
    );

    // Should have module facade creation
    assert!(
        generated_code.contains("types.ModuleType"),
        "Should create module facades"
    );

    // Should have initialization calls
    assert!(
        generated_code.contains("__cribo_init"),
        "Should have init calls"
    );
}

#[test]
fn test_multi_module_with_dependencies() {
    let config_code = r#"
DEBUG = True
API_KEY = "secret"
"#;

    let database_code = r#"
import config

def connect():
    if config.DEBUG:
        print("Debug mode enabled")
    return "Connected"
"#;

    let api_code = r#"
import config
import database

def make_request():
    conn = database.connect()
    return f"API request with key: {config.API_KEY[:4]}..."
"#;

    let config_ast = parse_python(config_code).expect("Failed to parse config");
    let database_ast = parse_python(database_code).expect("Failed to parse database");
    let api_ast = parse_python(api_code).expect("Failed to parse api");

    let mut bundler = StaticBundler::new();

    // Create module nodes
    let config_node = ModuleNode {
        name: "config".to_string(),
        path: PathBuf::from("config.py"),
        imports: vec![],
    };

    let database_node = ModuleNode {
        name: "database".to_string(),
        path: PathBuf::from("database.py"),
        imports: vec!["config".to_string()],
    };

    let api_node = ModuleNode {
        name: "api".to_string(),
        path: PathBuf::from("api.py"),
        imports: vec!["config".to_string(), "database".to_string()],
    };

    let sorted_nodes = vec![&config_node, &database_node, &api_node];

    // Bundle the modules
    let modules = vec![
        ("config".to_string(), config_ast),
        ("database".to_string(), database_ast),
        ("api".to_string(), api_ast),
    ];

    let bundled_ast = bundler
        .bundle_modules(modules, &sorted_nodes)
        .expect("Failed to bundle modules");

    // Verify wrapper classes exist for non-entry modules
    // Entry module (api) should not be wrapped
    let wrapper_count = bundled_ast.body.iter().filter(|stmt| {
        matches!(stmt, Stmt::ClassDef(class_def) if class_def.name.as_str().starts_with("__cribo_module_"))
    }).count();

    assert_eq!(
        wrapper_count, 2,
        "Should have 2 wrapper classes (config and database, but not api)"
    );

    let generated_code = generate_python(&bundled_ast);

    // Verify imports are rewritten
    assert!(
        !generated_code.contains("import config"),
        "Config import should be rewritten"
    );
    assert!(
        !generated_code.contains("import database"),
        "Database import should be rewritten"
    );
}

#[test]
fn test_module_with_initialization_code() {
    let module_code = r#"
# Module with initialization code
print("Loading module...")

counter = 0
items = []

if __name__ != "__main__":
    counter += 1
    items.append("initialized")

def get_counter():
    return counter
"#;

    let module_ast = parse_python(module_code).expect("Failed to parse module");

    let mut bundler = StaticBundler::new();
    let wrapped = bundler
        .wrap_module("test_module", module_ast)
        .expect("Failed to wrap module");

    // Check that we have __cribo_init method
    let class_body = match &wrapped.transformed_ast.body[0] {
        Stmt::ClassDef(class_def) => &class_def.body,
        _ => panic!("Expected class definition"),
    };

    let has_cribo_init = class_body.iter().any(
        |stmt| matches!(stmt, Stmt::FunctionDef(func) if func.name.as_str() == "__cribo_init"),
    );

    assert!(
        has_cribo_init,
        "Should have __cribo_init method for initialization code"
    );
}

#[test]
fn test_nested_module_structure() {
    let models_user_code = r#"
class User:
    def __init__(self, name):
        self.name = name
"#;

    let models_init_code = r#"
from .user import User

__all__ = ["User"]
"#;

    let main_code = r#"
from models import User

user = User("Alice")
print(user.name)
"#;

    let models_user_ast = parse_python(models_user_code).expect("Failed to parse models.user");
    let models_init_ast = parse_python(models_init_code).expect("Failed to parse models.__init__");
    let main_ast = parse_python(main_code).expect("Failed to parse main");

    let mut bundler = StaticBundler::new();

    // Create module nodes
    let models_user_node = ModuleNode {
        name: "models.user".to_string(),
        path: PathBuf::from("models/user.py"),
        imports: vec![],
    };

    let models_node = ModuleNode {
        name: "models".to_string(),
        path: PathBuf::from("models/__init__.py"),
        imports: vec!["models.user".to_string()],
    };

    let main_node = ModuleNode {
        name: "main".to_string(),
        path: PathBuf::from("main.py"),
        imports: vec!["models".to_string()],
    };

    let sorted_nodes = vec![&models_user_node, &models_node, &main_node];

    // Bundle the modules
    let modules = vec![
        ("models.user".to_string(), models_user_ast),
        ("models".to_string(), models_init_ast),
        ("main".to_string(), main_ast),
    ];

    let bundled_ast = bundler
        .bundle_modules(modules, &sorted_nodes)
        .expect("Failed to bundle modules");

    // Check that nested module structure is created
    let generated_code = generate_python(&bundled_ast);

    // Should create nested module structure
    assert!(
        generated_code.contains("models"),
        "Should have models module"
    );

    // Should handle relative imports properly
    assert!(
        generated_code.contains("__cribo_module_models_user"),
        "Should have models.user wrapper"
    );
}
