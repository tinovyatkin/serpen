# Cribo Semantic Analysis Implementation

## Overview

This document describes the evolving implementation of semantic analysis in cribo. The system has progressed from basic symbol tracking through AST-based conflict detection to sophisticated import resolution and module inlining with proper symbol renaming.

**Status**: 🔄 **IN PROGRESS** - Advanced features implemented, entry module renaming pending

## Actual Implementation

### Core Components

The implemented semantic analysis system uses a simplified but effective approach that leverages AST analysis for symbol conflict detection.

```rust
// crates/cribo/src/semantic_bundler.rs
/// Semantic bundler that analyzes symbol conflicts across modules
pub struct SemanticBundler {
    /// Module-specific semantic models
    module_semantics: FxIndexMap<ModuleId, ModuleSemanticInfo>,
    /// Global symbol registry
    global_symbols: SymbolRegistry,
    /// Typing modules for semantic analysis (unused in current implementation)
    typing_modules: Vec<String>,
}

/// Semantic information for a single module
pub struct ModuleSemanticInfo {
    /// Symbols exported by this module
    pub exported_symbols: FxIndexSet<String>,
    /// Symbol conflicts detected in this module
    pub conflicts: Vec<String>,
}

/// Global symbol registry across all modules
pub struct SymbolRegistry {
    /// Symbol name -> list of modules that define it
    symbols: FxIndexMap<String, Vec<ModuleId>>,
    /// Renames: (ModuleId, OriginalName) -> NewName
    renames: FxIndexMap<(ModuleId, String), String>,
}
```

### Symbol Extraction Strategy

Instead of full semantic model building, the implementation uses a targeted AST-based approach:

```rust
/// Extract symbols directly from AST without complex semantic model manipulation
struct SimpleSymbolExtractor;

impl SimpleSymbolExtractor {
    /// Extract module-level symbols from statements
    fn extract_symbols(stmts: &[Stmt]) -> FxIndexSet<String> {
        let mut symbols = FxIndexSet::default();

        for stmt in stmts {
            match stmt {
                Stmt::ClassDef(class_def) => {
                    // Skip private classes (start with underscore but not dunder)
                    if !class_def.name.starts_with('_') || class_def.name.starts_with("__") {
                        symbols.insert(class_def.name.to_string());
                    }
                }
                Stmt::FunctionDef(func_def) => {
                    // Skip private functions (start with underscore but not dunder)
                    if !func_def.name.starts_with('_') || func_def.name.starts_with("__") {
                        symbols.insert(func_def.name.to_string());
                    }
                }
                Stmt::Assign(assign) => {
                    // Extract variable assignments at module level
                    for target in &assign.targets {
                        if let ruff_python_ast::Expr::Name(name_expr) = target {
                            // Only include public variables (not starting with underscore)
                            if !name_expr.id.starts_with('_') {
                                symbols.insert(name_expr.id.to_string());
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        symbols
    }
}
```

## Integration with Code Generation

### Conflict Detection and Resolution

The system detects symbol conflicts across all modules and generates deterministic renames:

```rust
impl SymbolRegistry {
    /// Detect conflicts across all modules
    pub fn detect_conflicts(&self) -> Vec<SymbolConflict> {
        let mut conflicts = Vec::new();

        for (symbol, modules) in &self.symbols {
            if modules.len() > 1 {
                conflicts.push(SymbolConflict {
                    symbol: symbol.clone(),
                    modules: modules.clone(),
                });
            }
        }

        conflicts
    }

    /// Generate rename for conflicting symbol
    pub fn generate_rename(
        &mut self,
        module_id: ModuleId,
        original: &str,
        suffix: usize,
    ) -> String {
        let new_name = format!("{}_{}", original, suffix);
        self.renames
            .insert((module_id, original.to_string()), new_name.clone());
        new_name
    }
}
```

### Type Annotation Rewriting

**🎯 KEY ACHIEVEMENT**: The system correctly handles type annotations in function signatures, which was the primary issue causing `NameError: name 'Connection' is not defined`:

```rust
// In code_generator.rs - rewrite_symbols_in_stmt_with_scope
Stmt::FunctionDef(func_def) => {
    // Rewrite type annotations with parent scope (they're evaluated outside the function)
    for param in &mut func_def.parameters.posonlyargs {
        if let Some(ref mut annotation) = param.parameter.annotation {
            self.rewrite_symbols_in_expr_with_locals(annotation, rename_map, parent_locals);
        }
    }
    for param in &mut func_def.parameters.args {
        if let Some(ref mut annotation) = param.parameter.annotation {
            self.rewrite_symbols_in_expr_with_locals(annotation, rename_map, parent_locals);
        }
    }
    // ... handle kwonlyargs, vararg, kwarg annotations
    
    // Rewrite return type annotation with parent scope
    if let Some(ref mut returns) = func_def.returns {
        self.rewrite_symbols_in_expr_with_locals(returns, rename_map, parent_locals);
    }
}
```

### Scope-Aware Symbol Rewriting

The system implements proper scope tracking to prevent incorrect renaming of local variables:

```rust
fn rewrite_symbols_in_expr_with_locals(
    &self,
    expr: &mut Expr,
    rename_map: &FxIndexMap<String, String>,
    local_vars: &FxIndexSet<String>,
) {
    match expr {
        Expr::Name(name_expr) => {
            // Only rename if the variable is not a local variable
            let var_name = name_expr.id.as_str();
            if !local_vars.contains(var_name) {
                if let Some(new_name) = rename_map.get(var_name) {
                    name_expr.id = new_name.clone().into();
                }
            }
        } // ... handle other expression types recursively
    }
}
```

## Implementation Journey

### Challenges Overcome

1. **Type Annotation Bug**:
   - **Problem**: Function return type annotations like `-> Connection` were not being renamed, causing `NameError`
   - **Solution**: Extended function definition rewriting to handle all annotation types (parameters, return types, vararg, kwarg)

2. **Module-Specific Renames**:
   - **Problem**: Symbol renaming was hardcoded to only apply to the "main" module
   - **Solution**: Created `apply_module_renames_to_stmt()` to apply module-specific renames to inlined modules

3. **Scope Tracking**:
   - **Problem**: Local variables were being incorrectly renamed, breaking Python semantics
   - **Solution**: Implemented comprehensive scope tracking with `collect_local_assignments()` and scope-aware rewriting

4. **Import Alias Resolution**:
   - **Problem**: Aliases like `UserLogger` from `from .user import Logger as UserLogger` were not resolved
   - **Solution**: Implemented import alias tracking and resolution throughout AST traversal

5. **Self-Assignment Ordering**:
   - **Problem**: `process = process` was becoming `process_2 = process_2` causing NameError
   - **Solution**: Apply existing renames to RHS before creating new rename for LHS

6. **Relative Import Resolution**:
   - **Problem**: Complex relative imports like `from ...models import base` failed
   - **Solution**: Enhanced relative import resolution with special handling for packages and root modules

7. **Module Namespace Imports**:
   - **Problem**: `from ...models import base` where `base` is an inlined module failed
   - **Solution**: Create `types.SimpleNamespace` objects populated with renamed symbols

8. **Dynamic Symbol Access**:
   - **Problem**: `globals()["Logger"]` returned string instead of renamed class
   - **Solution**: Rewrite string literals in globals() subscript expressions

9. **Import Alias Forward References**:
   - **Problem**: Import aliases couldn't resolve symbols from not-yet-inlined modules
   - **Solution**: Store as "module:symbol" format for later resolution when available

### Testing Results

Before the fix:

```
NameError: name 'Connection' is not defined. Did you mean: 'Connection_1'?
```

After the fix:

```python
def connect(User: "User") -> Connection_1:  # ✅ Type annotation correctly renamed
```

The semantic analysis successfully detected and resolved 8 symbol conflicts:

- `Connection` (6 modules)
- `User` (5 modules)
- `Logger` (6 modules)
- `result` (8 modules)
- `connection` (5 modules)
- `process` (7 modules)
- `validate` (7 modules)
- `connect` (5 modules)

## Current Implementation Details

### Advanced Features Implemented

#### 1. Import Alias Resolution

The system now tracks and resolves import aliases across module boundaries:

```rust
// Track import aliases during module inlining
let actual_name = if self.inlined_modules.contains(&resolved) {
    if let Some(source_renames) = ctx.module_renames.get(&resolved) {
        if let Some(renamed) = source_renames.get(imported_name) {
            renamed.clone()  // Use the renamed symbol directly
        } else {
            imported_name.to_string()
        }
    } else {
        // Module not yet inlined, store for later resolution
        format!("{}:{}", resolved, imported_name)
    }
} else {
    imported_name.to_string()
};
ctx.import_aliases.insert(local_name.to_string(), actual_name);
```

#### 2. Module Import Namespace Creation

For imports of inlined modules (e.g., `from ...models import base`), the system creates namespace objects:

```rust
// Create namespace object for inlined module
body.push(Stmt::Assign(StmtAssign {
    targets: vec![Expr::Name(ExprName {
        id: local_name.into(),
        ctx: ExprContext::Store,
        range: TextRange::default(),
    })],
    value: Box::new(Expr::Call(ExprCall {
        func: Box::new(Expr::Attribute(ExprAttribute {
            value: Box::new(Expr::Name(ExprName {
                id: "types".into(),
                ctx: ExprContext::Load,
                range: TextRange::default(),
            })),
            attr: Identifier::new("SimpleNamespace", TextRange::default()),
            ctx: ExprContext::Load,
            range: TextRange::default(),
        })),
        // ...
    })),
    // ...
}));

// Add symbols to namespace
for (original_name, renamed_name) in module_renames {
    // base.original_name = renamed_name
    body.push(/* assignment to namespace attribute */);
}
```

#### 3. Dynamic globals() Access Rewriting

Handles patterns like `globals()["Logger"]` that access renamed symbols:

```rust
// Special handling for globals()["name"] patterns
if let Expr::Call(call) = subscript_expr.value.as_ref() {
    if let Expr::Name(name) = call.func.as_ref() {
        if name.id == "globals" && call.arguments.args.is_empty() {
            if let Expr::StringLiteral(string_lit) = subscript_expr.slice.as_ref() {
                let key = string_lit.value.to_string();
                if let Some(canonical) = alias_to_canonical.get(&key) {
                    // Rewrite the string literal to the renamed value
                    subscript_expr.slice = Box::new(Expr::StringLiteral(/* renamed */));
                }
            }
        }
    }
}
```

#### 4. Comprehensive Import Alias Resolution in Statements

Extended to handle all statement types including FunctionDef and ClassDef:

```rust
Stmt::FunctionDef(func_def) => {
    // Resolve in parameter defaults and annotations
    for param in &mut func_def.parameters.args {
        if let Some(ref mut default) = param.default {
            self.resolve_import_aliases_in_expr(default, import_aliases);
        }
        if let Some(ref mut annotation) = param.parameter.annotation {
            self.resolve_import_aliases_in_expr(annotation, import_aliases);
        }
    }
    // Resolve in return type annotation
    if let Some(ref mut returns) = func_def.returns {
        self.resolve_import_aliases_in_expr(returns, import_aliases);
    }
    // Resolve in function body
    for stmt in &mut func_def.body {
        self.resolve_import_aliases_in_stmt(stmt, import_aliases);
    }
}
```

### ✅ Completed Features

1. **Symbol Conflict Detection**: Correctly identifies conflicts across all modules
2. **Deterministic Renaming**: Generates consistent `symbol_N` renames
3. **Type Annotation Support**: Handles function parameter and return type annotations
4. **Scope-Aware Rewriting**: Prevents incorrect renaming of local variables
5. **Module-Specific Application**: Applies correct renames to each module
6. **Import Alias Tracking**: Resolves aliases like `from .user import Logger as UserLogger`
7. **Module Namespace Objects**: Creates `types.SimpleNamespace` for imported inlined modules
8. **Relative Import Resolution**: Handles complex relative imports across package boundaries
9. **Dynamic Symbol Access**: Rewrites `globals()["name"]` patterns for renamed symbols
10. **Wrapper Module Import Resolution**: Properly resolves imports from inlined modules in wrapper modules

### 🔄 In Progress

1. **Entry Module Symbol Renaming**: The entry module's local definitions that conflict with imports are not renamed
   - Example: `Logger = "string_logger"` in main.py conflicts with imported `Logger` class
   - Current behavior preserves Python semantics but causes runtime errors in tests

### 📋 Future Enhancements

1. **Entry Module Conflict Resolution**: Rename symbols in entry module that conflict with imports
2. **Full ruff_python_semantic Integration**: Leverage complete semantic model for advanced features
3. **Dead Code Elimination**: Remove unused imports and symbols
4. **Type Hint Stripping**: Optional removal of type annotations for production
5. **Minification**: Symbol shortening and whitespace removal

## Architecture Decisions

### Why Simplified AST Approach?

The implementation uses direct AST analysis rather than full `ruff_python_semantic` integration for several reasons:

1. **Immediate Results**: Solved the critical type annotation issue quickly
2. **Maintainability**: Simpler code that's easier to debug and extend
3. **Performance**: Lightweight analysis focused on specific bundling needs
4. **Incremental Migration**: Can gradually adopt more semantic features

### Future Migration Path

The current implementation provides a solid foundation for eventual full semantic integration:

1. **Phase 1**: ✅ Basic conflict detection and type annotation support
2. **Phase 2**: 🔄 Complete module-level symbol resolution
3. **Phase 3**: 📋 Advanced scope analysis using full semantic model
4. **Phase 4**: 📋 Optimization features (dead code elimination, minification)

## Key Implementation Files

### crates/cribo/src/semantic_bundler.rs

- **SemanticBundler**: Main coordinator for semantic analysis
- **SymbolRegistry**: Tracks conflicts and generates renames
- **SimpleSymbolExtractor**: AST-based symbol extraction

### crates/cribo/src/code_generator.rs

- **Type annotation rewriting**: Enhanced function definition handling
- **Scope-aware symbol rewriting**: Prevents local variable conflicts
- **Module-specific rename application**: Applies correct renames per module

## Benefits Achieved

1. **✅ Correct Type Annotation Handling**: Fixed critical `NameError` with function return types
2. **✅ Deterministic Symbol Conflicts**: Consistent renaming across all modules
3. **✅ Scope Awareness**: Prevents incorrect local variable renaming
4. **✅ Future-Proof Architecture**: Foundation for advanced semantic features
5. **✅ Maintainable Code**: Clear separation of concerns and targeted fixes

## Testing and Validation

The implementation successfully processes the comprehensive_ast_rewrite fixture with complex naming conflicts:

- **8 symbol conflicts detected** across 12 modules
- **Type annotations correctly renamed** (e.g., `-> Connection` becomes `-> Connection_1`)
- **Scope tracking working** for most variable shadowing cases
- **Deterministic output** with consistent symbol numbering
- **Import aliases resolved** (e.g., `UserLogger` → `Logger_1`)
- **Module namespaces created** for inlined module imports
- **Dynamic access patterns handled** (e.g., `globals()["Logger"]` → `globals()["Logger_1"]`)

### Current Test Status

The comprehensive_ast_rewrite test is progressing significantly:

- **Original failure**: Line 266 (early in execution)
- **Current failure**: Line 509 or later (much deeper in execution)
- **Remaining issue**: Entry module symbol conflicts not renamed

Example of remaining issue:

```python
# In main.py:
from models.user import Logger  # Imports Logger class
Logger = "string_logger"  # Overwrites with string
# Later in main():
model_logger = Logger("model")  # TypeError: 'str' object is not callable
```

This represents a major step forward in cribo's bundling accuracy and robustness, with most complex symbol resolution cases now handled correctly.
