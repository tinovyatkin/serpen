use anyhow::Result;
use insta::assert_snapshot;
use serpen::bundler::Bundler;
use serpen::config::Config;
use std::path::PathBuf;
use tempfile::TempDir;

fn bundle_test_script(script_dir: &str) -> Result<String> {
    let script_dir_path = PathBuf::from("tests/fixtures/circular_dependencies").join(script_dir);
    let main_path = script_dir_path.join("main.py");

    if !main_path.exists() {
        panic!("Main script not found: {}", main_path.display());
    }

    let config = Config {
        src: vec![script_dir_path],
        ..Default::default()
    };
    let mut bundler = Bundler::new(config);

    let temp_dir = TempDir::new()?;
    let output_path = temp_dir.path().join("bundled_script.py");

    // Bundle the script
    bundler.bundle(&main_path, &output_path, false)?;

    // Read the bundled content
    let bundled_content = std::fs::read_to_string(&output_path)?;
    Ok(bundled_content)
}

#[test]
fn test_three_module_circular_dependency() {
    let result = bundle_test_script("three_module_cycle");

    // This should detect the circular dependency: module_a -> module_b -> module_c -> module_a
    match result {
        Ok(bundled_content) => {
            // If bundling succeeds, it means we've successfully resolved the circular dependency
            // This is a function-level cycle that should be resolvable
            assert_snapshot!("three_module_cycle_bundled", bundled_content);
        }
        Err(error) => {
            // If it fails, we should get a meaningful error about the circular dependency
            let error_msg = error.to_string();
            assert!(
                error_msg.contains("Circular dependencies detected")
                    || error_msg.contains("circular")
                    || error_msg.contains("cycle"),
                "Error should mention circular dependency: {}",
                error_msg
            );
            assert_snapshot!("three_module_cycle_error", error_msg);
        }
    }
}

#[test]
fn test_four_module_circular_dependency() {
    let result = bundle_test_script("four_module_cycle");

    // This tests a longer cycle: A -> B -> C -> D -> A
    match result {
        Ok(bundled_content) => {
            assert_snapshot!("four_module_cycle_bundled", bundled_content);
        }
        Err(error) => {
            let error_msg = error.to_string();
            assert!(
                error_msg.contains("Circular dependencies detected")
                    || error_msg.contains("circular")
                    || error_msg.contains("cycle"),
                "Error should mention circular dependency: {}",
                error_msg
            );
            assert_snapshot!("four_module_cycle_error", error_msg);
        }
    }
}

#[test]
fn test_package_level_circular_dependency() {
    let result = bundle_test_script("package_level_cycles");

    // Tests circular dependency between packages: pkg1 -> pkg2 -> pkg1
    match result {
        Ok(bundled_content) => {
            assert_snapshot!("package_level_cycles_bundled", bundled_content);
        }
        Err(error) => {
            let error_msg = error.to_string();
            assert!(
                error_msg.contains("Circular dependencies detected")
                    || error_msg.contains("circular")
                    || error_msg.contains("cycle"),
                "Error should mention circular dependency: {}",
                error_msg
            );
            assert_snapshot!("package_level_cycles_error", error_msg);
        }
    }
}

#[test]
fn test_relative_import_circular_dependency() {
    let result = bundle_test_script("relative_import_cycles");

    // Tests circular dependency with relative imports: .auth -> .database -> .auth
    match result {
        Ok(bundled_content) => {
            assert_snapshot!("relative_import_cycles_bundled", bundled_content);
        }
        Err(error) => {
            let error_msg = error.to_string();
            assert!(
                error_msg.contains("Circular dependencies detected")
                    || error_msg.contains("circular")
                    || error_msg.contains("cycle"),
                "Error should mention circular dependency: {}",
                error_msg
            );
            assert_snapshot!("relative_import_cycles_error", error_msg);
        }
    }
}

#[test]
fn test_unresolvable_circular_dependency() {
    let result = bundle_test_script("unresolvable_patterns");

    // This should always fail - temporal paradox with module-level constants
    match result {
        Ok(bundled_content) => {
            panic!(
                "Unresolvable circular dependency should not bundle successfully. Got: {}",
                bundled_content
            );
        }
        Err(error) => {
            let error_msg = error.to_string();
            assert!(
                error_msg.contains("Circular dependencies detected")
                    || error_msg.contains("circular")
                    || error_msg.contains("cycle"),
                "Error should mention circular dependency: {}",
                error_msg
            );
            assert_snapshot!("unresolvable_patterns_error", error_msg);
        }
    }
}

#[test]
fn test_circular_dependency_detection_in_dependency_graph() {
    use serpen::dependency_graph::{DependencyGraph, ModuleNode};
    use std::path::PathBuf;

    let mut graph = DependencyGraph::new();

    // Create a simple circular dependency: A -> B -> A
    let module_a = ModuleNode {
        name: "module_a".to_string(),
        path: PathBuf::from("/test/module_a.py"),
        imports: vec!["module_b".to_string()],
    };

    let module_b = ModuleNode {
        name: "module_b".to_string(),
        path: PathBuf::from("/test/module_b.py"),
        imports: vec!["module_a".to_string()],
    };

    graph.add_module(module_a);
    graph.add_module(module_b);
    graph.add_dependency("module_a", "module_b").unwrap();
    graph.add_dependency("module_b", "module_a").unwrap();

    // The graph should detect the cycle
    assert!(
        graph.has_cycles(),
        "Graph should detect circular dependency"
    );

    // Topological sort should fail with cycle information
    let sort_result = graph.topological_sort();
    assert!(
        sort_result.is_err(),
        "Topological sort should fail on circular dependency"
    );

    let error_msg = sort_result.unwrap_err().to_string();
    assert!(
        error_msg.contains("Circular dependency detected"),
        "Error should mention circular dependency detection: {}",
        error_msg
    );
}

#[test]
fn test_tarjans_strongly_connected_components() {
    use serpen::dependency_graph::{DependencyGraph, ModuleNode};
    use std::path::PathBuf;

    let mut graph = DependencyGraph::new();

    // Create a more complex graph with multiple cycles
    // Cycle 1: A -> B -> A
    // Cycle 2: C -> D -> E -> C
    // Single node: F

    let modules = vec![
        ("module_a", vec!["module_b"]),
        ("module_b", vec!["module_a"]),
        ("module_c", vec!["module_d"]),
        ("module_d", vec!["module_e"]),
        ("module_e", vec!["module_c"]),
        ("module_f", vec![]),
    ];

    // Add all modules
    for (name, imports) in &modules {
        let module = ModuleNode {
            name: name.to_string(),
            path: PathBuf::from(format!("/test/{}.py", name)),
            imports: imports.iter().map(|s| s.to_string()).collect(),
        };
        graph.add_module(module);
    }

    // Add dependencies
    for (from, imports) in &modules {
        for to in imports {
            graph.add_dependency(from, to).unwrap();
        }
    }

    // Find strongly connected components
    let sccs = graph.find_strongly_connected_components();

    // Should find 2 SCCs (the two cycles)
    assert_eq!(
        sccs.len(),
        2,
        "Should find exactly 2 strongly connected components"
    );

    // Each SCC should have the right number of modules
    let mut scc_sizes: Vec<usize> = sccs.iter().map(|scc| scc.len()).collect();
    scc_sizes.sort();
    assert_eq!(
        scc_sizes,
        vec![2, 3],
        "Should have one 2-module cycle and one 3-module cycle"
    );
}

#[test]
fn test_cycle_path_detection() {
    use serpen::dependency_graph::{DependencyGraph, ModuleNode};
    use std::path::PathBuf;

    let mut graph = DependencyGraph::new();

    // Create a simple 3-module cycle: A -> B -> C -> A
    let modules = vec![
        ("module_a", vec!["module_b"]),
        ("module_b", vec!["module_c"]),
        ("module_c", vec!["module_a"]),
    ];

    for (name, imports) in &modules {
        let module = ModuleNode {
            name: name.to_string(),
            path: PathBuf::from(format!("/test/{}.py", name)),
            imports: imports.iter().map(|s| s.to_string()).collect(),
        };
        graph.add_module(module);
    }

    for (from, imports) in &modules {
        for to in imports {
            graph.add_dependency(from, to).unwrap();
        }
    }

    // Find cycle paths
    let cycle_paths = graph.find_cycle_paths().unwrap();

    // Should find at least one cycle
    assert!(
        !cycle_paths.is_empty(),
        "Should find at least one cycle path"
    );

    // Each cycle should have 3 modules
    for cycle in &cycle_paths {
        assert!(
            cycle.len() >= 3,
            "Cycle should have at least 3 modules: {:?}",
            cycle
        );
    }
}

#[test]
fn test_circular_dependency_classification() {
    use serpen::dependency_graph::{CircularDependencyType, DependencyGraph, ModuleNode};
    use std::path::PathBuf;

    let mut graph = DependencyGraph::new();

    // Create cycles with different types
    // Function-level cycle: auth -> database -> auth
    // Constants cycle: constants_a -> constants_b -> constants_a

    let modules = vec![
        ("auth", vec!["database"]),
        ("database", vec!["auth"]),
        ("constants_a", vec!["constants_b"]),
        ("constants_b", vec!["constants_a"]),
    ];

    for (name, imports) in &modules {
        let module = ModuleNode {
            name: name.to_string(),
            path: PathBuf::from(format!("/test/{}.py", name)),
            imports: imports.iter().map(|s| s.to_string()).collect(),
        };
        graph.add_module(module);
    }

    for (from, imports) in &modules {
        for to in imports {
            graph.add_dependency(from, to).unwrap();
        }
    }

    // Classify circular dependencies
    let analysis = graph.classify_circular_dependencies();

    // Should have 2 cycles total
    assert_eq!(analysis.total_cycles_detected, 2, "Should detect 2 cycles");

    // Should have 1 resolvable and 1 unresolvable cycle
    assert_eq!(
        analysis.resolvable_cycles.len(),
        1,
        "Should have 1 resolvable cycle"
    );
    assert_eq!(
        analysis.unresolvable_cycles.len(),
        1,
        "Should have 1 unresolvable cycle"
    );

    // Check that constants cycle is classified as unresolvable
    let unresolvable_cycle = &analysis.unresolvable_cycles[0];
    assert!(matches!(
        unresolvable_cycle.cycle_type,
        CircularDependencyType::ModuleConstants
    ));
    assert!(
        unresolvable_cycle
            .modules
            .iter()
            .any(|m| m.contains("constants"))
    );

    // Check that auth cycle is classified as resolvable
    let resolvable_cycle = &analysis.resolvable_cycles[0];
    assert!(matches!(
        resolvable_cycle.cycle_type,
        CircularDependencyType::FunctionLevel
    ));
}
