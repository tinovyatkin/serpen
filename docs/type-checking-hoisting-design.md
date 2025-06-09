# TYPE_CHECKING Import Hoisting and Deduplication System Design

## Executive Summary

This document outlines the design and implementation strategy for hoisting and deduplicating imports gated by `if TYPE_CHECKING:` blocks in Serpen's bundled output. The goal is to create clean, optimized Python bundles that preserve type-checking capabilities while avoiding runtime overhead and import duplication.

## Problem Statement

When bundling Python modules, TYPE_CHECKING-gated imports present unique challenges:

1. **Scattered Imports**: TYPE_CHECKING blocks appear throughout modules, creating scattered import statements
2. **Duplication**: The same type-only import may appear in multiple TYPE_CHECKING blocks
3. **Mixed Context**: Some imports exist both inside and outside TYPE_CHECKING blocks
4. **Bundling Complexity**: Inlined modules each have their own TYPE_CHECKING blocks
5. **Runtime Safety**: TYPE_CHECKING imports must remain gated to avoid runtime import errors

### Example Problem

```python
# Input: Multiple modules with TYPE_CHECKING imports
# module_a.py
from typing import TYPE_CHECKING
if TYPE_CHECKING:
    from .types import UserType
    from third_party import ValidationError

# module_b.py  
from typing import TYPE_CHECKING
if TYPE_CHECKING:
    from .types import UserType  # Duplicate!
    from another_lib import AsyncClient

# Current bundling might produce:
from typing import TYPE_CHECKING
# ... module_a code with TYPE_CHECKING block ...
# ... module_b code with TYPE_CHECKING block ...
# Results in duplicate imports and scattered TYPE_CHECKING blocks
```

## Solution Overview

The TYPE_CHECKING hoisting system will:

1. **Collect** all TYPE_CHECKING-gated imports during bundling
2. **Deduplicate** imports across all bundled modules
3. **Hoist** all TYPE_CHECKING imports to a single block at the bundle top
4. **Preserve** runtime safety by keeping the TYPE_CHECKING gate
5. **Optimize** by removing empty TYPE_CHECKING blocks from inlined code

### Desired Output

```python
# Bundled output with hoisted TYPE_CHECKING imports
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from .types import UserType
    from another_lib import AsyncClient
    from third_party import ValidationError

# ... rest of bundled code without TYPE_CHECKING blocks ...
```

## Technical Design

### Core Architecture

```rust
use indexmap::{IndexMap, IndexSet};
use ruff_python_ast::{self as ast, visitor::preorder::PreorderVisitor};

pub struct TypeCheckingHoister {
    /// Collected TYPE_CHECKING imports from all modules
    type_checking_imports: TypeCheckingImports,

    /// Regular runtime imports for deduplication check
    runtime_imports: RuntimeImports,

    /// Import classification for proper handling
    import_classifier: ImportClassifier,

    /// Configuration
    config: HoistingConfig,
}

#[derive(Default)]
pub struct TypeCheckingImports {
    /// Direct imports: `import module`
    direct_imports: IndexMap<String, ImportInfo>,

    /// From imports: `from module import name`
    from_imports: IndexMap<String, FromImportInfo>,

    /// Preserve order of discovery for deterministic output
    discovery_order: Vec<ImportKey>,
}

#[derive(Clone, Debug)]
pub struct ImportInfo {
    /// The import statement AST node
    stmt: ast::StmtImport,

    /// Source module where this was found
    source_module: String,

    /// Whether this import has an alias
    alias: Option<String>,

    /// Classification (FirstParty, ThirdParty, StandardLibrary)
    classification: ImportClassification,
}

#[derive(Clone, Debug)]
pub struct FromImportInfo {
    /// The module being imported from
    module: String,

    /// Names being imported with their aliases
    names: IndexMap<String, Option<String>>,

    /// Source modules where found
    source_modules: IndexSet<String>,

    /// Classification
    classification: ImportClassification,

    /// Import level for relative imports
    level: u32,
}
```

### Import Collection Phase

```rust
pub struct TypeCheckingVisitor<'a> {
    /// Currently processing TYPE_CHECKING block
    in_type_checking: bool,

    /// Stack of TYPE_CHECKING conditions for nested ifs
    type_checking_stack: Vec<TypeCheckingContext>,

    /// Collected imports
    imports: &'a mut TypeCheckingImports,

    /// Current module being processed
    current_module: String,
}

impl<'a> PreorderVisitor<'_> for TypeCheckingVisitor<'a> {
    fn visit_stmt(&mut self, stmt: &ast::Stmt) {
        match stmt {
            ast::Stmt::If(if_stmt) => {
                if self.is_type_checking_condition(&if_stmt.test) {
                    // Enter TYPE_CHECKING context
                    self.type_checking_stack.push(TypeCheckingContext {
                        condition: if_stmt.test.clone(),
                        has_else: !if_stmt.elif_else_clauses.is_empty(),
                    });
                    self.in_type_checking = true;

                    // Visit body
                    for stmt in &if_stmt.body {
                        self.visit_stmt(stmt);
                    }

                    // Exit context
                    self.type_checking_stack.pop();
                    self.in_type_checking = !self.type_checking_stack.is_empty();

                    // Don't visit elif/else - they're runtime branches
                    return;
                }
            }
            ast::Stmt::Import(import) if self.in_type_checking => {
                self.collect_import(import);
                return;
            }
            ast::Stmt::ImportFrom(import_from) if self.in_type_checking => {
                self.collect_import_from(import_from);
                return;
            }
            _ => {}
        }

        // Continue traversal
        walk_stmt(self, stmt);
    }
}

impl<'a> TypeCheckingVisitor<'a> {
    fn is_type_checking_condition(&self, expr: &ast::Expr) -> bool {
        match expr {
            // Direct: if TYPE_CHECKING:
            ast::Expr::Name(name) => name.id == "TYPE_CHECKING",

            // Attribute: if typing.TYPE_CHECKING:
            ast::Expr::Attribute(attr) => {
                attr.attr == "TYPE_CHECKING" && self.is_typing_module(&attr.value)
            }

            // Handle aliased imports: if TC: (where TC = TYPE_CHECKING)
            ast::Expr::Name(name) => self.import_tracker.is_type_checking_alias(&name.id),

            _ => false,
        }
    }

    fn collect_import_from(&mut self, import_from: &ast::StmtImportFrom) {
        let module = import_from
            .module
            .as_ref()
            .map(|m| m.to_string())
            .unwrap_or_else(|| self.resolve_relative_import(import_from.level));

        let entry = self
            .imports
            .from_imports
            .entry(module.clone())
            .or_insert_with(|| FromImportInfo {
                module: module.clone(),
                names: IndexMap::new(),
                source_modules: IndexSet::new(),
                classification: self.classify_import(&module),
                level: import_from.level,
            });

        // Add all imported names
        for alias in &import_from.names {
            let name = alias.name.to_string();
            let alias_name = alias.asname.as_ref().map(|a| a.to_string());

            // Update or insert, preserving first occurrence's alias
            entry.names.entry(name).or_insert(alias_name);
        }

        // Track source module
        entry.source_modules.insert(self.current_module.clone());

        // Track discovery order
        self.imports.discovery_order.push(ImportKey::From(module));
    }
}
```

### Deduplication Logic

```rust
pub struct ImportDeduplicator {
    /// Strategy for handling conflicts
    conflict_strategy: ConflictStrategy,
}

#[derive(Debug, Clone)]
pub enum ConflictStrategy {
    /// Keep first occurrence
    KeepFirst,
    /// Keep last occurrence  
    KeepLast,
    /// Merge aliases (import both)
    MergeAliases,
    /// Error on conflicts
    ErrorOnConflict,
}

impl ImportDeduplicator {
    pub fn deduplicate(&self, imports: &mut TypeCheckingImports) -> Result<(), DeduplicationError> {
        // Deduplicate from imports
        self.deduplicate_from_imports(&mut imports.from_imports)?;

        // Deduplicate direct imports
        self.deduplicate_direct_imports(&mut imports.direct_imports)?;

        // Cross-check with runtime imports
        self.check_runtime_conflicts(imports)?;

        Ok(())
    }

    fn deduplicate_from_imports(
        &self,
        from_imports: &mut IndexMap<String, FromImportInfo>,
    ) -> Result<(), DeduplicationError> {
        for (module, info) in from_imports.iter_mut() {
            // Sort names for deterministic output
            let mut sorted_names: Vec<_> = info.names.iter().collect();
            sorted_names.sort_by_key(|(name, _)| name.as_str());

            // Rebuild with sorted names
            info.names = sorted_names
                .into_iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();

            // Check for alias conflicts
            let mut seen_names: IndexMap<String, String> = IndexMap::new();
            for (name, alias) in &info.names {
                let import_as = alias.as_ref().unwrap_or(name);

                if let Some(existing) = seen_names.get(import_as) {
                    match self.conflict_strategy {
                        ConflictStrategy::ErrorOnConflict => {
                            return Err(DeduplicationError::AliasConflict {
                                module: module.clone(),
                                name: import_as.clone(),
                                sources: vec![existing.clone(), name.clone()],
                            });
                        }
                        ConflictStrategy::MergeAliases => {
                            // Keep both with modified aliases
                            // This requires updating the alias to make it unique
                        }
                        _ => {} // KeepFirst/KeepLast handled by insertion order
                    }
                } else {
                    seen_names.insert(import_as.clone(), name.clone());
                }
            }
        }

        Ok(())
    }
}
```

### Hoisting Implementation

```rust
pub struct TypeCheckingHoistingTransformer {
    /// Collected TYPE_CHECKING imports
    hoisted_imports: TypeCheckingImports,

    /// Tracks which TYPE_CHECKING blocks to remove
    blocks_to_remove: Vec<ast::Location>,

    /// Configuration
    config: HoistingConfig,
}

impl TypeCheckingHoistingTransformer {
    pub fn transform_module(&mut self, module: &mut ast::ModModule) -> Result<(), TransformError> {
        // Phase 1: Collect all TYPE_CHECKING imports
        let mut collector = TypeCheckingVisitor::new(&mut self.hoisted_imports);
        collector.visit_module(module);

        // Phase 2: Deduplicate imports
        let deduplicator = ImportDeduplicator::new(self.config.conflict_strategy);
        deduplicator.deduplicate(&mut self.hoisted_imports)?;

        // Phase 3: Remove TYPE_CHECKING blocks from module body
        self.remove_type_checking_blocks(module);

        // Phase 4: Insert hoisted TYPE_CHECKING block at top
        self.insert_hoisted_block(module);

        Ok(())
    }

    fn remove_type_checking_blocks(&mut self, module: &mut ast::ModModule) {
        let mut remover = TypeCheckingBlockRemover {
            blocks_to_remove: &self.blocks_to_remove,
        };

        module.body = module
            .body
            .drain(..)
            .filter_map(|stmt| remover.transform_statement(stmt))
            .collect();
    }

    fn insert_hoisted_block(&self, module: &mut ast::ModModule) {
        if self.hoisted_imports.is_empty() {
            return;
        }

        let mut new_body = Vec::new();
        let mut insert_position = 0;

        // Preserve module docstring
        if let Some(ast::Stmt::Expr(expr)) = module.body.first() {
            if matches!(expr.value, ast::Expr::StringLiteral(_)) {
                new_body.push(module.body[0].clone());
                insert_position = 1;
            }
        }

        // Preserve __future__ imports
        while insert_position < module.body.len() {
            if let ast::Stmt::ImportFrom(import) = &module.body[insert_position] {
                if import.module.as_ref().map(|m| m.as_str()) == Some("__future__") {
                    new_body.push(module.body[insert_position].clone());
                    insert_position += 1;
                    continue;
                }
            }
            break;
        }

        // Insert TYPE_CHECKING import if not present
        if !self.has_type_checking_import(&module.body[insert_position..]) {
            new_body.push(self.create_type_checking_import());
        }

        // Insert hoisted TYPE_CHECKING block
        new_body.push(self.create_hoisted_block());

        // Append rest of module
        new_body.extend(module.body[insert_position..].iter().cloned());

        module.body = new_body;
    }

    fn create_hoisted_block(&self) -> ast::Stmt {
        let mut body = Vec::new();

        // Add imports in deterministic order
        for key in &self.hoisted_imports.discovery_order {
            match key {
                ImportKey::Direct(name) => {
                    if let Some(info) = self.hoisted_imports.direct_imports.get(name) {
                        body.push(ast::Stmt::Import(info.stmt.clone()));
                    }
                }
                ImportKey::From(module) => {
                    if let Some(info) = self.hoisted_imports.from_imports.get(module) {
                        body.push(self.create_from_import_stmt(info));
                    }
                }
            }
        }

        // Create if TYPE_CHECKING: block
        ast::Stmt::If(ast::StmtIf {
            test: Box::new(ast::Expr::Name(ast::ExprName {
                id: "TYPE_CHECKING".to_string(),
                ctx: ast::ExprContext::Load,
                range: ast::TextRange::default(),
            })),
            body,
            elif_else_clauses: vec![],
            range: ast::TextRange::default(),
        })
    }

    fn create_from_import_stmt(&self, info: &FromImportInfo) -> ast::Stmt {
        let names = info
            .names
            .iter()
            .map(|(name, alias)| ast::Alias {
                name: ast::Identifier::new(name.clone(), ast::TextRange::default()),
                asname: alias
                    .as_ref()
                    .map(|a| ast::Identifier::new(a.clone(), ast::TextRange::default())),
                range: ast::TextRange::default(),
            })
            .collect();

        ast::Stmt::ImportFrom(ast::StmtImportFrom {
            module: Some(ast::Identifier::new(
                info.module.clone(),
                ast::TextRange::default(),
            )),
            names: names,
            level: info.level,
            range: ast::TextRange::default(),
        })
    }
}
```

### Integration with Bundler

```rust
// In bundler.rs
impl Bundler {
    pub fn bundle_with_type_checking_hoisting(&mut self) -> Result<String, BundleError> {
        // Step 1: Regular bundling logic to collect modules
        let modules = self.collect_modules()?;

        // Step 2: Create combined AST
        let mut combined_ast = self.combine_modules(modules)?;

        // Step 3: Apply TYPE_CHECKING hoisting
        if self.config.hoist_type_checking_imports {
            let mut hoister = TypeCheckingHoistingTransformer::new(&self.config);
            hoister.transform_module(&mut combined_ast)?;
        }

        // Step 4: Apply other transformations (unused imports, etc.)
        self.apply_post_transformations(&mut combined_ast)?;

        // Step 5: Generate final output
        self.generate_output(combined_ast)
    }
}

// Configuration
#[derive(Debug, Clone)]
pub struct HoistingConfig {
    /// Enable TYPE_CHECKING import hoisting
    pub hoist_type_checking_imports: bool,

    /// How to handle import conflicts
    pub conflict_strategy: ConflictStrategy,

    /// Preserve relative import structure
    pub preserve_relative_imports: bool,

    /// Sort imports within TYPE_CHECKING block
    pub sort_imports: bool,

    /// Group imports by classification
    pub group_by_classification: bool,
}
```

### Edge Case Handling

```rust
pub struct EdgeCaseHandler {
    /// Handle complex TYPE_CHECKING conditions
    pub fn handle_complex_conditions(&self, expr: &ast::Expr) -> bool {
        match expr {
            // if TYPE_CHECKING or DEBUG:
            ast::Expr::BoolOp(bool_op) => {
                match &bool_op.op {
                    ast::BoolOp::Or => {
                        // Any operand being TYPE_CHECKING makes it type-checking
                        bool_op.values.iter().any(|v| self.is_type_checking_condition(v))
                    }
                    ast::BoolOp::And => {
                        // All operands must include TYPE_CHECKING
                        bool_op.values.iter().any(|v| self.is_type_checking_condition(v))
                    }
                }
            }
            
            // if not TYPE_CHECKING: (inverse - skip the if body, process else)
            ast::Expr::UnaryOp(unary_op) => {
                matches!(unary_op.op, ast::UnaryOp::Not) && 
                self.is_type_checking_condition(&unary_op.operand)
            }
            
            _ => false,
        }
    }
    
    /// Handle nested TYPE_CHECKING blocks
    pub fn handle_nested_blocks(&mut self, stmts: &[ast::Stmt]) -> Vec<ast::Stmt> {
        // Flatten nested TYPE_CHECKING blocks
        let mut result = Vec::new();
        
        for stmt in stmts {
            if let ast::Stmt::If(if_stmt) = stmt {
                if self.is_type_checking_condition(&if_stmt.test) {
                    // Extract body and recursively process
                    result.extend(self.handle_nested_blocks(&if_stmt.body));
                    continue;
                }
            }
            result.push(stmt.clone());
        }
        
        result
    }
    
    /// Handle mixed runtime/type-checking imports
    pub fn separate_mixed_imports(&self, module: &ast::ModModule) 
        -> (Vec<ast::Stmt>, Vec<ast::Stmt>) {
        let mut runtime_imports = Vec::new();
        let mut type_checking_imports = Vec::new();
        
        // Track which imports are used at runtime vs type-checking only
        let usage_analyzer = ImportUsageAnalyzer::new(module);
        let runtime_used = usage_analyzer.find_runtime_used_imports();
        
        for stmt in &module.body {
            match stmt {
                ast::Stmt::Import(import) => {
                    if runtime_used.contains(&import.names[0].name) {
                        runtime_imports.push(stmt.clone());
                    } else {
                        type_checking_imports.push(stmt.clone());
                    }
                }
                ast::Stmt::ImportFrom(import_from) => {
                    let module_name = import_from.module.as_ref()
                        .map(|m| m.to_string())
                        .unwrap_or_default();
                    
                    // Split names by usage
                    let (runtime_names, type_names): (Vec<_>, Vec<_>) = 
                        import_from.names.iter()
                        .partition(|alias| runtime_used.contains(&alias.name));
                    
                    if !runtime_names.is_empty() {
                        runtime_imports.push(self.create_import_with_names(
                            import_from, runtime_names
                        ));
                    }
                    
                    if !type_names.is_empty() {
                        type_checking_imports.push(self.create_import_with_names(
                            import_from, type_names
                        ));
                    }
                }
                _ => {}
            }
        }
        
        (runtime_imports, type_checking_imports)
    }
}
```

## Implementation Guidelines

### Phase 1: Foundation

1. **Import Collection Infrastructure**
   ```rust
   // Start with basic TYPE_CHECKING detection
   impl TypeCheckingDetector {
       pub fn is_type_checking_if(&self, if_stmt: &ast::StmtIf) -> bool;
       pub fn extract_imports(&self, body: &[ast::Stmt]) -> Vec<ImportEntry>;
   }
   ```

2. **Basic Deduplication**
   - Implement simple name-based deduplication
   - Handle direct imports and from imports separately
   - Create deterministic ordering system

3. **Test Infrastructure**
   ```python
   # Test fixture example
   # test_type_checking_hoisting.py
   from typing import TYPE_CHECKING

   if TYPE_CHECKING:
       from typing import Dict, List
       from .models import User

   def process(data):
       pass

   if TYPE_CHECKING:
       from typing import Dict  # Duplicate!
       from .views import View
   ```

### Phase 2: Core Implementation

1. **AST Transformation**
   - Implement TYPE_CHECKING block removal
   - Create hoisted block insertion logic
   - Handle edge cases (nested blocks, complex conditions)

2. **Integration with Bundler**
   - Add configuration options
   - Integrate with existing bundling pipeline
   - Ensure compatibility with other transformations

3. **Import Classification**
   - Leverage existing FirstParty/ThirdParty classification
   - Group imports appropriately in hoisted block
   - Maintain relative import relationships

### Phase 3: Advanced Features

1. **Conflict Resolution**
   - Implement multiple conflict strategies
   - Handle alias conflicts intelligently
   - Provide clear error messages

2. **Optimization**
   - Remove empty TYPE_CHECKING blocks
   - Consolidate multiple TYPE_CHECKING conditions
   - Optimize import ordering for readability

3. **Edge Case Handling**
   - Complex boolean conditions
   - Nested TYPE_CHECKING blocks
   - Mixed runtime/type-checking usage

### Phase 4: Testing and Polish

1. **Comprehensive Testing**
   ```rust
   #[test]
   fn test_basic_hoisting() {
       let input = include_str!("fixtures/type_checking/basic.py");
       let expected = include_str!("fixtures/type_checking/basic_expected.py");
       assert_eq!(hoist_type_checking(input), expected);
   }

   #[test]
   fn test_deduplication() {
       // Test with duplicate imports across modules
   }

   #[test]
   fn test_complex_conditions() {
       // Test with OR, AND, NOT conditions
   }
   ```

2. **Performance Optimization**
   - Benchmark with large codebases
   - Optimize AST traversal
   - Minimize memory allocations

3. **Documentation**
   - User guide for configuration options
   - Examples of before/after transformations
   - Troubleshooting guide

## Configuration Schema

```toml
[bundler.type_checking]
# Enable TYPE_CHECKING import hoisting
hoist_imports = true

# How to handle duplicate imports with different aliases
# Options: "keep_first", "keep_last", "merge_aliases", "error"
conflict_strategy = "keep_first"

# Sort imports within TYPE_CHECKING block
sort_imports = true

# Group imports by type (standard library, third party, first party)
group_imports = true

# Preserve relative import levels
preserve_relative_imports = true

# Remove empty TYPE_CHECKING blocks after hoisting
remove_empty_blocks = true

# Custom TYPE_CHECKING aliases to recognize
type_checking_aliases = ["TC", "TYPE_CHECK"]
```

## Testing Strategy

### Unit Tests

1. **TYPE_CHECKING Detection**
   - Simple `if TYPE_CHECKING:`
   - `if typing.TYPE_CHECKING:`
   - Aliased conditions
   - Complex boolean expressions

2. **Import Collection**
   - Direct imports
   - From imports with aliases
   - Relative imports
   - Star imports handling

3. **Deduplication**
   - Exact duplicates
   - Same module, different names
   - Alias conflicts
   - Cross-module duplicates

### Integration Tests

1. **Real-world Patterns**
   ```python
   # Test with actual library patterns
   # Pydantic-style TYPE_CHECKING usage
   if TYPE_CHECKING:
       from typing import Any, Callable, Dict
       from pydantic.typing import AnyCallable
       from .validators import ValidatorFunc

   # FastAPI-style TYPE_CHECKING usage
   if TYPE_CHECKING:
       from starlette.requests import Request
       from starlette.responses import Response
   ```

2. **Bundle Output Validation**
   - Syntactically valid Python
   - Preserved functionality
   - No runtime import errors
   - Proper type checker compatibility

### Snapshot Tests

```rust
use insta::assert_snapshot;

#[test]
fn test_comprehensive_hoisting() {
    let fixtures = vec![
        "simple_type_checking",
        "nested_blocks",
        "mixed_imports",
        "complex_conditions",
        "multi_module",
    ];

    for fixture in fixtures {
        let input = read_fixture(&format!("{}.py", fixture));
        let output = hoist_type_checking_imports(&input);
        assert_snapshot!(format!("hoisting_{}", fixture), output);
    }
}
```

## Success Metrics

1. **Correctness**
   - All TYPE_CHECKING imports preserved
   - No runtime import errors
   - Type checkers still work correctly

2. **Performance**
   - <100ms overhead for typical bundles
   - Linear scaling with codebase size
   - Minimal memory footprint

3. **Code Quality**
   - Clean, readable hoisted imports
   - Deterministic output
   - Proper deduplication

4. **User Experience**
   - Clear configuration options
   - Helpful error messages
   - Seamless integration

## Future Enhancements

1. **Smart Import Grouping**
   - Group related imports together
   - Preserve logical import organization
   - Custom grouping rules

2. **Type Stub Generation**
   - Extract TYPE_CHECKING imports to .pyi files
   - Support for partial type stub generation
   - Integration with existing stub files

3. **Import Optimization**
   - Remove truly unused TYPE_CHECKING imports
   - Convert to lazy imports where beneficial
   - Minimize import overhead

4. **IDE Integration**
   - Preserve source mapping for IDEs
   - Support for jump-to-definition
   - Debugging support

## Conclusion

The TYPE_CHECKING import hoisting and deduplication system will significantly improve the quality of Serpen's bundled output by consolidating type-checking imports into a single, clean block at the top of the bundle. This design provides a robust foundation for handling the complexities of Python's type system while maintaining runtime safety and type checker compatibility.
