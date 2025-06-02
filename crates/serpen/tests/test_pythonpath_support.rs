use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

use serpen::config::Config;
use serpen::resolver::ModuleResolver;

#[test]
fn test_pythonpath_module_discovery() {
    // Create temporary directories for testing
    let temp_dir = TempDir::new().unwrap();
    let pythonpath_dir = temp_dir.path().join("pythonpath_modules");
    let src_dir = temp_dir.path().join("src");

    // Create directory structures
    fs::create_dir_all(&pythonpath_dir).unwrap();
    fs::create_dir_all(&src_dir).unwrap();

    // Create a module in PYTHONPATH directory
    let pythonpath_module = pythonpath_dir.join("pythonpath_module.py");
    fs::write(
        &pythonpath_module,
        "# This is a PYTHONPATH module\ndef hello():\n    return 'Hello from PYTHONPATH'",
    )
    .unwrap();

    // Create a package in PYTHONPATH directory
    let pythonpath_pkg = pythonpath_dir.join("pythonpath_pkg");
    fs::create_dir_all(&pythonpath_pkg).unwrap();
    let pythonpath_pkg_init = pythonpath_pkg.join("__init__.py");
    fs::write(&pythonpath_pkg_init, "# PYTHONPATH package").unwrap();
    let pythonpath_pkg_module = pythonpath_pkg.join("submodule.py");
    fs::write(&pythonpath_pkg_module, "# PYTHONPATH submodule").unwrap();

    // Create a module in src directory
    let src_module = src_dir.join("src_module.py");
    fs::write(&src_module, "# This is a src module").unwrap();

    // Set up config with src directory
    let config = Config {
        src: vec![src_dir.clone()],
        ..Default::default()
    };

    // Set PYTHONPATH environment variable
    let pythonpath_str = pythonpath_dir.to_string_lossy();
    unsafe {
        std::env::set_var("PYTHONPATH", pythonpath_str.as_ref());
    }

    // Create resolver and test module discovery
    let resolver = ModuleResolver::new(config).unwrap();
    let first_party_modules = resolver.get_first_party_modules();

    // Verify that modules from both src and PYTHONPATH are discovered
    assert!(
        first_party_modules.contains("src_module"),
        "Should discover modules from configured src directories"
    );
    assert!(
        first_party_modules.contains("pythonpath_module"),
        "Should discover modules from PYTHONPATH directories"
    );
    assert!(
        first_party_modules.contains("pythonpath_pkg"),
        "Should discover packages from PYTHONPATH directories"
    );
    assert!(
        first_party_modules.contains("pythonpath_pkg.submodule"),
        "Should discover submodules from PYTHONPATH packages"
    );

    // Clean up environment
    unsafe {
        std::env::remove_var("PYTHONPATH");
    }
}

#[test]
fn test_pythonpath_module_classification() {
    // Create temporary directories for testing
    let temp_dir = TempDir::new().unwrap();
    let pythonpath_dir = temp_dir.path().join("pythonpath_modules");
    let src_dir = temp_dir.path().join("src");

    // Create directory structures
    fs::create_dir_all(&pythonpath_dir).unwrap();
    fs::create_dir_all(&src_dir).unwrap();

    // Create a module in PYTHONPATH directory
    let pythonpath_module = pythonpath_dir.join("pythonpath_module.py");
    fs::write(&pythonpath_module, "# This is a PYTHONPATH module").unwrap();

    // Set up config
    let config = Config {
        src: vec![src_dir.clone()],
        ..Default::default()
    };

    // Set PYTHONPATH environment variable
    let pythonpath_str = pythonpath_dir.to_string_lossy();
    unsafe {
        std::env::set_var("PYTHONPATH", pythonpath_str.as_ref());
    }

    // Create resolver and test module classification
    let resolver = ModuleResolver::new(config).unwrap();

    // Test that PYTHONPATH modules are classified as first-party
    use serpen::resolver::ImportType;
    assert_eq!(
        resolver.classify_import("pythonpath_module"),
        ImportType::FirstParty,
        "PYTHONPATH modules should be classified as first-party"
    );

    // Test that unknown modules are still classified as third-party
    assert_eq!(
        resolver.classify_import("unknown_module"),
        ImportType::ThirdParty,
        "Unknown modules should still be classified as third-party"
    );

    // Clean up environment
    unsafe {
        std::env::remove_var("PYTHONPATH");
    }
}

#[test]
fn test_pythonpath_multiple_directories() {
    // Create temporary directories for testing
    let temp_dir = TempDir::new().unwrap();
    let pythonpath_dir1 = temp_dir.path().join("pythonpath1");
    let pythonpath_dir2 = temp_dir.path().join("pythonpath2");
    let src_dir = temp_dir.path().join("src");

    // Create directory structures
    fs::create_dir_all(&pythonpath_dir1).unwrap();
    fs::create_dir_all(&pythonpath_dir2).unwrap();
    fs::create_dir_all(&src_dir).unwrap();

    // Create modules in different PYTHONPATH directories
    let module1 = pythonpath_dir1.join("module1.py");
    fs::write(&module1, "# Module in pythonpath1").unwrap();

    let module2 = pythonpath_dir2.join("module2.py");
    fs::write(&module2, "# Module in pythonpath2").unwrap();

    // Set up config
    let config = Config {
        src: vec![src_dir.clone()],
        ..Default::default()
    };

    // Set PYTHONPATH with multiple directories (colon-separated)
    let pythonpath_str = format!(
        "{}:{}",
        pythonpath_dir1.to_string_lossy(),
        pythonpath_dir2.to_string_lossy()
    );
    unsafe {
        std::env::set_var("PYTHONPATH", &pythonpath_str);
    }

    // Create resolver and test module discovery
    let resolver = ModuleResolver::new(config).unwrap();
    let first_party_modules = resolver.get_first_party_modules();

    // Verify that modules from both PYTHONPATH directories are discovered
    assert!(
        first_party_modules.contains("module1"),
        "Should discover modules from first PYTHONPATH directory"
    );
    assert!(
        first_party_modules.contains("module2"),
        "Should discover modules from second PYTHONPATH directory"
    );

    // Clean up environment
    unsafe {
        std::env::remove_var("PYTHONPATH");
    }
}

#[test]
fn test_pythonpath_empty_or_nonexistent() {
    // Set up config
    let config = Config {
        src: vec![PathBuf::from("tests/fixtures/simple_project")],
        ..Default::default()
    };

    // Test with empty PYTHONPATH
    unsafe {
        std::env::set_var("PYTHONPATH", "");
    }

    let resolver1 = ModuleResolver::new(config.clone()).unwrap();
    let scan_dirs1 = resolver1.get_scan_directories();

    // Should only contain configured src directories
    assert_eq!(scan_dirs1.len(), 1);
    assert!(scan_dirs1.contains(&PathBuf::from("tests/fixtures/simple_project")));

    // Test with no PYTHONPATH
    unsafe {
        std::env::remove_var("PYTHONPATH");
    }

    let resolver2 = ModuleResolver::new(config.clone()).unwrap();
    let scan_dirs2 = resolver2.get_scan_directories();

    // Should only contain configured src directories
    assert_eq!(scan_dirs2.len(), 1);
    assert!(scan_dirs2.contains(&PathBuf::from("tests/fixtures/simple_project")));

    // Test with nonexistent directories in PYTHONPATH
    unsafe {
        std::env::set_var("PYTHONPATH", "/nonexistent1:/nonexistent2");
    }

    let resolver3 = ModuleResolver::new(config).unwrap();
    let scan_dirs3 = resolver3.get_scan_directories();

    // Should only contain configured src directories (nonexistent dirs filtered out)
    assert_eq!(scan_dirs3.len(), 1);
    assert!(scan_dirs3.contains(&PathBuf::from("tests/fixtures/simple_project")));

    // Clean up environment
    unsafe {
        std::env::remove_var("PYTHONPATH");
    }
}
