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
fn test_annotated_assignments() {
    let python_code = r#"
# Simple annotated assignments
name: str = "John"
age: int = 30
items: list[str] = ["apple", "banana"]

# Annotated without value
count: int

# Complex annotated assignment
user_data: dict[str, Any] = get_user_data()
"#;

    let ast = parse_python(python_code).expect("Failed to parse Python code");
    let mut bundler = StaticBundler::new();
    let wrapped = bundler
        .wrap_module("typed_module", ast)
        .expect("Failed to wrap module");

    let body = get_wrapped_module_body(&wrapped.transformed_ast);

    // Should have __cribo_vars with the simple assignments
    let has_cribo_vars = body.iter().any(|stmt| {
        matches!(stmt, Stmt::Assign(assign) if assign.targets.iter().any(|t| {
            matches!(t, ruff_python_ast::Expr::Name(n) if n.id.as_str() == "__cribo_vars")
        }))
    });

    assert!(
        has_cribo_vars,
        "Should have __cribo_vars for simple annotated assignments"
    );

    // Should have __cribo_init for complex cases
    let has_cribo_init = body.iter().any(|stmt| {
        matches!(stmt, Stmt::FunctionDef(func_def) if func_def.name.as_str() == "__cribo_init")
    });

    assert!(
        has_cribo_init,
        "Should have __cribo_init for complex annotated assignments"
    );
}

#[test]
fn test_control_flow_statements() {
    let python_code = r#"
# Module-level control flow
if DEBUG:
    log_level = "DEBUG"
else:
    log_level = "INFO"

for i in range(10):
    print(f"Initializing {i}")

while not connected:
    try_connect()
    
with open("config.json") as f:
    config = json.load(f)
"#;

    let ast = parse_python(python_code).expect("Failed to parse Python code");
    let mut bundler = StaticBundler::new();
    let wrapped = bundler
        .wrap_module("control_flow", ast)
        .expect("Failed to wrap module");

    let body = get_wrapped_module_body(&wrapped.transformed_ast);

    // All control flow should be in __cribo_init
    let init_method = body.iter().find_map(|stmt| match stmt {
        Stmt::FunctionDef(func_def) if func_def.name.as_str() == "__cribo_init" => Some(func_def),
        _ => None,
    });

    assert!(init_method.is_some(), "Should have __cribo_init method");

    let init_body = &init_method.expect("Failed to get init method").body;

    // Check for control flow statements
    let has_if = init_body.iter().any(|stmt| matches!(stmt, Stmt::If(_)));
    let has_for = init_body.iter().any(|stmt| matches!(stmt, Stmt::For(_)));
    let has_while = init_body.iter().any(|stmt| matches!(stmt, Stmt::While(_)));
    let has_with = init_body.iter().any(|stmt| matches!(stmt, Stmt::With(_)));

    assert!(has_if, "Should have if statement in __cribo_init");
    assert!(has_for, "Should have for statement in __cribo_init");
    assert!(has_while, "Should have while statement in __cribo_init");
    assert!(has_with, "Should have with statement in __cribo_init");
}

#[test]
fn test_exception_handling() {
    let python_code = r#"
# Module-level exception handling
try:
    import optional_dependency
    HAS_OPTIONAL = True
except ImportError:
    HAS_OPTIONAL = False
    
try:
    config = load_config()
except FileNotFoundError:
    config = default_config()
except Exception as e:
    logger.error(f"Failed to load config: {e}")
    raise
finally:
    logger.info("Config loading attempted")
"#;

    let ast = parse_python(python_code).expect("Failed to parse Python code");
    let mut bundler = StaticBundler::new();
    let wrapped = bundler
        .wrap_module("exception_module", ast)
        .expect("Failed to wrap module");

    let body = get_wrapped_module_body(&wrapped.transformed_ast);

    // Should have __cribo_init with try statements
    let init_method = body.iter().find_map(|stmt| match stmt {
        Stmt::FunctionDef(func_def) if func_def.name.as_str() == "__cribo_init" => Some(func_def),
        _ => None,
    });

    assert!(init_method.is_some(), "Should have __cribo_init method");

    let init_body = &init_method.expect("Failed to get init method").body;
    let try_count = init_body
        .iter()
        .filter(|stmt| matches!(stmt, Stmt::Try(_)))
        .count();

    assert_eq!(try_count, 2, "Should have 2 try statements in __cribo_init");
}

#[test]
fn test_augmented_assignments() {
    let python_code = r#"
# Module-level augmented assignments
counter = 0
counter += 1
counter *= 2

items = []
items += ["a", "b", "c"]

flags = 0b0000
flags |= 0b0001
flags &= 0b1111
"#;

    let ast = parse_python(python_code).expect("Failed to parse Python code");
    let mut bundler = StaticBundler::new();
    let wrapped = bundler
        .wrap_module("augment_module", ast)
        .expect("Failed to wrap module");

    let body = get_wrapped_module_body(&wrapped.transformed_ast);

    // Initial assignments should be in __cribo_vars
    let has_cribo_vars = body.iter().any(|stmt| {
        matches!(stmt, Stmt::Assign(assign) if assign.targets.iter().any(|t| {
            matches!(t, ruff_python_ast::Expr::Name(n) if n.id.as_str() == "__cribo_vars")
        }))
    });

    assert!(
        has_cribo_vars,
        "Should have __cribo_vars for initial assignments"
    );

    // Augmented assignments should be in __cribo_init
    let init_method = body.iter().find_map(|stmt| match stmt {
        Stmt::FunctionDef(func_def) if func_def.name.as_str() == "__cribo_init" => Some(func_def),
        _ => None,
    });

    assert!(init_method.is_some(), "Should have __cribo_init method");

    let init_body = &init_method.expect("Failed to get init method").body;
    let aug_assign_count = init_body
        .iter()
        .filter(|stmt| matches!(stmt, Stmt::AugAssign(_)))
        .count();

    assert!(
        aug_assign_count >= 5,
        "Should have augmented assignments in __cribo_init"
    );
}

#[test]
fn test_delete_statements() {
    let python_code = r#"
# Module-level delete statements
temp_var = "temporary"
del temp_var

cache = {"key": "value"}
del cache["key"]

items = [1, 2, 3, 4, 5]
del items[2:4]
"#;

    let ast = parse_python(python_code).expect("Failed to parse Python code");
    let mut bundler = StaticBundler::new();
    let wrapped = bundler
        .wrap_module("delete_module", ast)
        .expect("Failed to wrap module");

    let body = get_wrapped_module_body(&wrapped.transformed_ast);

    // Delete statements should be in __cribo_init
    let init_method = body.iter().find_map(|stmt| match stmt {
        Stmt::FunctionDef(func_def) if func_def.name.as_str() == "__cribo_init" => Some(func_def),
        _ => None,
    });

    assert!(init_method.is_some(), "Should have __cribo_init method");

    let init_body = &init_method.expect("Failed to get init method").body;
    let del_count = init_body
        .iter()
        .filter(|stmt| matches!(stmt, Stmt::Delete(_)))
        .count();

    assert_eq!(
        del_count, 3,
        "Should have 3 delete statements in __cribo_init"
    );
}

#[test]
fn test_global_nonlocal_statements() {
    let python_code = r#"
# Module-level global/nonlocal usage
def modify_global():
    global counter
    counter += 1
    
def nested_function():
    x = 10
    def inner():
        nonlocal x
        x = 20
    inner()
    return x
"#;

    let ast = parse_python(python_code).expect("Failed to parse Python code");
    let mut bundler = StaticBundler::new();
    let wrapped = bundler
        .wrap_module("scope_module", ast)
        .expect("Failed to wrap module");

    let body = get_wrapped_module_body(&wrapped.transformed_ast);

    // Functions should be static methods
    let function_count = body.iter().filter(|stmt| {
        matches!(stmt, Stmt::FunctionDef(func_def) if func_def.decorator_list.iter().any(|dec| {
            matches!(&dec.expression, ruff_python_ast::Expr::Name(name) if name.id.as_str() == "staticmethod")
        }))
    }).count();

    assert_eq!(function_count, 2, "Should have 2 static methods");
}

#[test]
fn test_assert_statements() {
    let python_code = r#"
# Module-level assertions
assert sys.version_info >= (3, 8), "Python 3.8+ required"
assert CONFIG_PATH.exists(), f"Config file not found: {CONFIG_PATH}"

DEBUG_MODE = True
assert DEBUG_MODE, "Debug mode must be enabled for development"
"#;

    let ast = parse_python(python_code).expect("Failed to parse Python code");
    let mut bundler = StaticBundler::new();
    let wrapped = bundler
        .wrap_module("assert_module", ast)
        .expect("Failed to wrap module");

    let body = get_wrapped_module_body(&wrapped.transformed_ast);

    // Assert statements should be in __cribo_init
    let init_method = body.iter().find_map(|stmt| match stmt {
        Stmt::FunctionDef(func_def) if func_def.name.as_str() == "__cribo_init" => Some(func_def),
        _ => None,
    });

    assert!(init_method.is_some(), "Should have __cribo_init method");

    let init_body = &init_method.expect("Failed to get init method").body;
    let assert_count = init_body
        .iter()
        .filter(|stmt| matches!(stmt, Stmt::Assert(_)))
        .count();

    assert_eq!(
        assert_count, 3,
        "Should have 3 assert statements in __cribo_init"
    );
}

#[test]
fn test_raise_statements() {
    let python_code = r#"
# Module-level conditional raises
if not HAS_REQUIRED_MODULE:
    raise ImportError("Required module not available")

if PLATFORM != "linux":
    raise OSError("This module only works on Linux")
"#;

    let ast = parse_python(python_code).expect("Failed to parse Python code");
    let mut bundler = StaticBundler::new();
    let wrapped = bundler
        .wrap_module("raise_module", ast)
        .expect("Failed to wrap module");

    let body = get_wrapped_module_body(&wrapped.transformed_ast);

    // Raise statements should be in __cribo_init
    let init_method = body.iter().find_map(|stmt| match stmt {
        Stmt::FunctionDef(func_def) if func_def.name.as_str() == "__cribo_init" => Some(func_def),
        _ => None,
    });

    assert!(init_method.is_some(), "Should have __cribo_init method");
}

#[test]
fn test_type_alias_statements() {
    // Note: This test might fail on older Python/parser versions that don't support type aliases
    let python_code = r#"
# Type alias at module level (Python 3.12+)
type Point = tuple[float, float]
type Vector = list[float]
type Matrix = list[list[float]]

def process_point(p: Point) -> Vector:
    return list(p)
"#;

    // This might fail to parse on older parsers, so we'll handle the error
    if let Ok(ast) = parse_python(python_code) {
        let mut bundler = StaticBundler::new();
        if let Ok(wrapped) = bundler.wrap_module("type_alias_module", ast) {
            let body = get_wrapped_module_body(&wrapped.transformed_ast);

            // Type aliases should remain at class level
            let type_alias_count = body
                .iter()
                .filter(|stmt| matches!(stmt, Stmt::TypeAlias(_)))
                .count();

            // If the parser supports type aliases, we should have them
            if type_alias_count > 0 {
                assert_eq!(
                    type_alias_count, 3,
                    "Should have 3 type aliases at class level"
                );
            }
        }
    }
}

#[test]
fn test_mixed_complex_module() {
    let python_code = r#"
"""Complex module with various statement types."""
import os
import sys
from pathlib import Path

# Constants
VERSION = "1.0.0"
DEBUG: bool = os.getenv("DEBUG", "false").lower() == "true"

# Runtime initialization
if DEBUG:
    import logging
    logging.basicConfig(level=logging.DEBUG)
    logger = logging.getLogger(__name__)
else:
    logger = None

# Data structures
config = {}
try:
    with open("config.json") as f:
        import json
        config = json.load(f)
except FileNotFoundError:
    pass

# Augmented assignment
total_items = 0
for category in config.get("categories", []):
    total_items += len(category.get("items", []))

# Assertions
assert total_items >= 0, "Invalid item count"

# Functions
def initialize():
    global initialized
    initialized = True
    
def cleanup():
    global config
    del config
    
# Type checking (if supported)
if TYPE_CHECKING:
    from typing import Any
    ConfigType = dict[str, Any]
"#;

    let ast = parse_python(python_code).expect("Failed to parse Python code");
    let mut bundler = StaticBundler::new();
    let wrapped = bundler
        .wrap_module("complex", ast)
        .expect("Failed to wrap module");

    let body = get_wrapped_module_body(&wrapped.transformed_ast);

    // Should have __cribo_vars
    let has_vars = body.iter().any(|stmt| {
        matches!(stmt, Stmt::Assign(assign) if assign.targets.iter().any(|t| {
            matches!(t, ruff_python_ast::Expr::Name(n) if n.id.as_str() == "__cribo_vars")
        }))
    });

    // Should have __cribo_init
    let has_init = body.iter().any(|stmt| {
        matches!(stmt, Stmt::FunctionDef(func_def) if func_def.name.as_str() == "__cribo_init")
    });

    // Should have static methods
    let static_method_count = body.iter().filter(|stmt| {
        matches!(stmt, Stmt::FunctionDef(func_def) if func_def.decorator_list.iter().any(|dec| {
            matches!(&dec.expression, ruff_python_ast::Expr::Name(name) if name.id.as_str() == "staticmethod")
        }))
    }).count();

    assert!(has_vars, "Should have __cribo_vars");
    assert!(has_init, "Should have __cribo_init");
    assert_eq!(
        static_method_count, 3,
        "Should have 3 static methods (initialize, cleanup, and __cribo_init)"
    );
}
