use std::collections::HashSet;
use serpen::emit::CodeEmitter;
use serpen::config::Config;
use serpen::resolver::ModuleResolver;

#[test]
fn test_rewrite_import_without_unused() {
    let config = Config::default();
    let resolver = ModuleResolver::new(config).unwrap();
    let emitter = CodeEmitter::new(resolver, false, false);
    
    // Test 1: Simple import with one unused
    let mut unused = HashSet::new();
    unused.insert("sys".to_string());
    
    let result = emitter.rewrite_import_without_unused("import os, sys", &unused);
    assert_eq!(result, Some("import os".to_string()));
    
    // Test 2: From import with mixed usage
    let mut unused = HashSet::new();
    unused.insert("Counter".to_string());
    
    let result = emitter.rewrite_import_without_unused("from collections import defaultdict, Counter", &unused);
    assert_eq!(result, Some("from collections import defaultdict".to_string()));
    
    // Test 3: All imports unused
    let mut unused = HashSet::new();
    unused.insert("os".to_string());
    unused.insert("sys".to_string());
    
    let result = emitter.rewrite_import_without_unused("import os, sys", &unused);
    assert_eq!(result, None);
    
    // Test 4: Import with inline comment
    let mut unused = HashSet::new();
    unused.insert("sys".to_string());
    
    let result = emitter.rewrite_import_without_unused("import os, sys  # System imports", &unused);
    assert_eq!(result, Some("import os  # System imports".to_string()));
    
    // Test 5: From import with comment and mixed usage
    let mut unused = HashSet::new();
    unused.insert("PurePath".to_string());
    
    let result = emitter.rewrite_import_without_unused("from pathlib import Path, PurePath  # Path utilities", &unused);
    assert_eq!(result, Some("from pathlib import Path  # Path utilities".to_string()));
    
    println!("All rewrite tests passed!");
}

fn main() {
    test_rewrite_import_without_unused();
}
