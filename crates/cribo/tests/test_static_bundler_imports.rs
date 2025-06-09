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
            // For bundled modules, rewrite_imports returns None (skipped)
            // For non-bundled modules, keep the original statement
            let is_bundled_import = matches!(&stmt, Stmt::Import(import) if import.names.iter().any(|alias|
                bundled_modules.contains(alias.name.as_str())
            ));

            if !is_bundled_import {
                rewritten_statements.push(stmt);
            }
        }
    }

    // Should have 2 statements: import os and import sys (separate non-bundled)
    // Bundled modules (models, utils) are skipped entirely since facades are pre-created
    println!("Rewritten statements count: {}", rewritten_statements.len());
    for (i, stmt) in rewritten_statements.iter().enumerate() {
        println!("Statement {}: {:?}", i, stmt);
    }
    assert!(
        rewritten_statements.len() == 2,
        "Should have 2 statements after rewriting (non-bundled imports only), got {}",
        rewritten_statements.len()
    );

    // Check that we only have non-bundled imports left
    let has_non_bundled_import = rewritten_statements
        .iter()
        .any(|stmt| matches!(stmt, Stmt::Import(_)));

    assert!(has_non_bundled_import, "Should have non-bundled imports");

    // Bundled modules should not appear in any import statements
    let has_bundled_in_imports = rewritten_statements.iter().any(|stmt| {
        matches!(stmt, Stmt::Import(import) if import.names.iter().any(|alias|
            bundled_modules.contains(alias.name.as_str())
        ))
    });

    assert!(
        !has_bundled_in_imports,
        "Bundled modules should not appear in import statements"
    );
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
            // For bundled modules, rewrite_imports returns None (skipped)
            // For non-bundled modules, keep the original statement
            let is_bundled_import = matches!(&stmt, Stmt::Import(import) if import.names.iter().any(|alias|
                bundled_modules.contains(alias.name.as_str())
            ));

            if !is_bundled_import {
                rewritten_statements.push(stmt);
            }
        }
    }

    // Should have 2 statements: numpy and utils imports (non-bundled)
    // The bundled 'models' import is skipped entirely
    assert!(
        rewritten_statements.len() == 2,
        "Should have 2 statements after rewriting (non-bundled imports only)"
    );

    // Check that we only have non-bundled imports
    let import_count = rewritten_statements
        .iter()
        .filter(|stmt| matches!(stmt, Stmt::Import(_)))
        .count();

    assert_eq!(
        import_count, 2,
        "Should have 2 non-bundled import statements"
    );

    // Bundled modules should not appear in any import statements
    let has_bundled_in_imports = rewritten_statements.iter().any(|stmt| {
        matches!(stmt, Stmt::Import(import) if import.names.iter().any(|alias|
            bundled_modules.contains(alias.name.as_str())
        ))
    });

    assert!(
        !has_bundled_in_imports,
        "Bundled modules should not appear in import statements"
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
        assignment_count >= 1,
        "Should have at least 1 assignment for bundled from-imports (User)"
    );

    // Check that bundled simple imports (models, utils) are not present
    let has_bundled_simple_imports = rewritten_statements.iter().any(|stmt| {
        matches!(stmt, Stmt::Import(import) if import.names.iter().any(|alias|
            bundled_modules.contains(alias.name.as_str())
        ))
    });

    assert!(
        !has_bundled_simple_imports,
        "Bundled simple imports should be skipped"
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
        }
        // Note: We don't add else clause here because bundled simple imports are skipped entirely
    }

    // Only from-imports should create assignments (for 'settings')
    // Simple imports (models, utils) are skipped entirely since facades are pre-created
    let assignment_count = rewritten_statements
        .iter()
        .filter(|stmt| matches!(stmt, Stmt::Assign(_)))
        .count();

    assert_eq!(
        assignment_count, 1,
        "Should have 1 assignment statement (for settings from config)"
    );

    assert_eq!(
        rewritten_statements.len(),
        1,
        "Should have 1 statement total (settings assignment)"
    );
}
