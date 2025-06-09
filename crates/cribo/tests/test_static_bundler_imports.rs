use anyhow::Result;
use cribo::static_bundler::StaticBundler;
use indexmap::IndexSet;
use ruff_python_ast::{ModModule, Stmt};
use ruff_python_parser::parse_module;

/// Helper function to parse Python code into an AST
fn parse_python(source: &str) -> Result<ModModule> {
    Ok(parse_module(source)?.into_syntax())
}

/// Helper to create a set of bundled module names
fn create_bundled_set(modules: &[&str]) -> IndexSet<String> {
    modules.iter().map(|&m| m.to_string()).collect()
}

#[test]
fn test_simple_import_rewriting() {
    let python_code = r#"
import os
import models
import sys
import utils
"#;

    let ast = parse_python(python_code).expect("Failed to parse Python code");
    let bundler = StaticBundler::new();
    let bundled_modules = create_bundled_set(&["models", "utils"]);

    // Process each statement
    let mut rewritten_statements = Vec::new();
    for stmt in ast.body {
        if let Some(rewritten) = bundler.rewrite_imports(&stmt, &bundled_modules) {
            rewritten_statements.extend(rewritten);
        } else {
            rewritten_statements.push(stmt);
        }
    }

    // Should have 4 statements:
    // 1. import os, sys (combined non-bundled)
    // 2. models = models (assignment)
    // 3. utils = utils (assignment)
    assert!(
        rewritten_statements.len() >= 3,
        "Should have at least 3 statements after rewriting"
    );

    // Check that we have assignments for bundled modules
    let has_models_assignment = rewritten_statements.iter().any(|stmt| {
        matches!(stmt, Stmt::Assign(assign) if assign.targets.iter().any(|t| {
            matches!(t, ruff_python_ast::Expr::Name(n) if n.id.as_str() == "models")
        }))
    });

    let has_utils_assignment = rewritten_statements.iter().any(|stmt| {
        matches!(stmt, Stmt::Assign(assign) if assign.targets.iter().any(|t| {
            matches!(t, ruff_python_ast::Expr::Name(n) if n.id.as_str() == "utils")
        }))
    });

    assert!(has_models_assignment, "Should have models assignment");
    assert!(has_utils_assignment, "Should have utils assignment");
}

#[test]
fn test_aliased_import_rewriting() {
    let python_code = r#"
import numpy as np
import models as m
import utils
"#;

    let ast = parse_python(python_code).expect("Failed to parse Python code");
    let bundler = StaticBundler::new();
    let bundled_modules = create_bundled_set(&["models"]);

    let mut rewritten_statements = Vec::new();
    for stmt in ast.body {
        if let Some(rewritten) = bundler.rewrite_imports(&stmt, &bundled_modules) {
            rewritten_statements.extend(rewritten);
        } else {
            rewritten_statements.push(stmt);
        }
    }

    // Check that aliased bundled import becomes assignment with alias
    let has_aliased_assignment = rewritten_statements.iter().any(|stmt| {
        matches!(stmt, Stmt::Assign(assign) if assign.targets.iter().any(|t| {
            matches!(t, ruff_python_ast::Expr::Name(n) if n.id.as_str() == "m")
        }))
    });

    assert!(
        has_aliased_assignment,
        "Should have aliased assignment for 'm'"
    );
}

#[test]
fn test_from_import_rewriting() {
    let python_code = r#"
from os import path
from models import User, Product
from utils import helper as h
"#;

    let ast = parse_python(python_code).expect("Failed to parse Python code");
    let bundler = StaticBundler::new();
    let bundled_modules = create_bundled_set(&["models", "utils"]);

    let mut rewritten_statements = Vec::new();
    for stmt in ast.body {
        if let Some(rewritten) = bundler.rewrite_imports(&stmt, &bundled_modules) {
            rewritten_statements.extend(rewritten);
        } else {
            rewritten_statements.push(stmt);
        }
    }

    // Should have getattr-based assignments for bundled modules
    let user_assignments = rewritten_statements
        .iter()
        .filter(|stmt| {
            matches!(stmt, Stmt::Assign(assign) if assign.targets.iter().any(|t| {
                matches!(t, ruff_python_ast::Expr::Name(n) if n.id.as_str() == "User")
            }))
        })
        .count();

    let product_assignments = rewritten_statements
        .iter()
        .filter(|stmt| {
            matches!(stmt, Stmt::Assign(assign) if assign.targets.iter().any(|t| {
                matches!(t, ruff_python_ast::Expr::Name(n) if n.id.as_str() == "Product")
            }))
        })
        .count();

    let helper_alias_assignments = rewritten_statements
        .iter()
        .filter(|stmt| {
            matches!(stmt, Stmt::Assign(assign) if assign.targets.iter().any(|t| {
                matches!(t, ruff_python_ast::Expr::Name(n) if n.id.as_str() == "h")
            }))
        })
        .count();

    assert_eq!(user_assignments, 1, "Should have User assignment");
    assert_eq!(product_assignments, 1, "Should have Product assignment");
    assert_eq!(
        helper_alias_assignments, 1,
        "Should have aliased helper assignment"
    );

    // Should still have the non-bundled from import
    let has_os_import = rewritten_statements
        .iter()
        .any(|stmt| matches!(stmt, Stmt::ImportFrom(_)));

    assert!(has_os_import, "Should still have os.path import");
}

#[test]
fn test_mixed_bundled_unbundled_imports() {
    let python_code = r#"
import os, sys, models, utils
from typing import List, Dict
from models import User
from collections import defaultdict
"#;

    let ast = parse_python(python_code).expect("Failed to parse Python code");
    let bundler = StaticBundler::new();
    let bundled_modules = create_bundled_set(&["models", "utils"]);

    let mut rewritten_statements = Vec::new();
    for stmt in ast.body {
        if let Some(rewritten) = bundler.rewrite_imports(&stmt, &bundled_modules) {
            rewritten_statements.extend(rewritten);
        } else {
            rewritten_statements.push(stmt);
        }
    }

    // Should have a mix of imports and assignments
    let import_count = rewritten_statements
        .iter()
        .filter(|stmt| matches!(stmt, Stmt::Import(_) | Stmt::ImportFrom(_)))
        .count();

    let assignment_count = rewritten_statements
        .iter()
        .filter(|stmt| matches!(stmt, Stmt::Assign(_)))
        .count();

    assert!(
        import_count >= 2,
        "Should have at least 2 import statements for non-bundled modules"
    );
    assert!(
        assignment_count >= 3,
        "Should have at least 3 assignments for bundled modules"
    );
}

#[test]
fn test_no_bundled_modules() {
    let python_code = r#"
import os
import sys
from typing import List
"#;

    let ast = parse_python(python_code).expect("Failed to parse Python code");
    let bundler = StaticBundler::new();
    let bundled_modules = IndexSet::new(); // Empty set

    let mut rewritten_statements = Vec::new();
    for stmt in ast.body {
        if let Some(rewritten) = bundler.rewrite_imports(&stmt, &bundled_modules) {
            rewritten_statements.extend(rewritten);
        } else {
            rewritten_statements.push(stmt);
        }
    }

    // All imports should remain unchanged
    assert_eq!(
        rewritten_statements.len(),
        3,
        "Should have same number of statements"
    );

    let all_imports = rewritten_statements
        .iter()
        .all(|stmt| matches!(stmt, Stmt::Import(_) | Stmt::ImportFrom(_)));

    assert!(all_imports, "All statements should still be imports");
}

#[test]
fn test_all_bundled_modules() {
    let python_code = r#"
import models
import utils
from config import settings
"#;

    let ast = parse_python(python_code).expect("Failed to parse Python code");
    let bundler = StaticBundler::new();
    let bundled_modules = create_bundled_set(&["models", "utils", "config"]);

    let mut rewritten_statements = Vec::new();
    for stmt in ast.body {
        if let Some(rewritten) = bundler.rewrite_imports(&stmt, &bundled_modules) {
            rewritten_statements.extend(rewritten);
        } else {
            rewritten_statements.push(stmt);
        }
    }

    // All imports should become assignments
    let all_assignments = rewritten_statements
        .iter()
        .all(|stmt| matches!(stmt, Stmt::Assign(_)));

    assert!(all_assignments, "All statements should be assignments");
    assert_eq!(
        rewritten_statements.len(),
        3,
        "Should have 3 assignment statements"
    );
}
