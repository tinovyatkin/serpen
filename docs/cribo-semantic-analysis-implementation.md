# Cribo Semantic Analysis Implementation

## Overview

This document describes the semantic analysis implementation in Cribo, which provides advanced Python code understanding capabilities for the bundling process. The semantic analysis system enables proper handling of variable scoping, name resolution, and module-level globals lifting.

## Architecture

### Core Components

#### 1. SemanticBundler (`semantic_bundler.rs`)

The `SemanticBundler` is the main orchestrator for semantic analysis, built on top of Ruff's semantic analysis infrastructure.

**Key Structures:**

```rust
pub struct SemanticBundler<'a> {
    db: SemanticDb<'a>,
    module_globals: FxIndexMap<ModuleId, ModuleGlobalInfo>,
    symbol_registry: SymbolRegistry,
    // ... other fields
}

pub struct ModuleGlobalInfo {
    pub module_level_vars: FxIndexSet<String>,
    pub global_declarations: FxIndexMap<String, Vec<TextRange>>,
    pub global_reads: FxIndexMap<String, Vec<TextRange>>,
    pub global_writes: FxIndexMap<String, Vec<TextRange>>,
    pub functions_using_globals: FxIndexSet<String>,
    pub module_name: String,
}
```

**Responsibilities:**

- Tracks module-level variable definitions
- Identifies global statements and their usage patterns
- Records which functions use global declarations
- Manages symbol renaming to avoid conflicts

#### 2. GlobalUsageVisitor

A visitor that traverses the AST to collect information about global variable usage:

```rust
struct GlobalUsageVisitor<'a> {
    info: &'a mut ModuleGlobalInfo,
    current_function: Option<String>,
    in_global_scope: bool,
}
```

**Key Behaviors:**

- Tracks module-level variable assignments
- Records global declarations within functions
- Identifies reads and writes to global variables
- Maintains context about current scope (function vs module level)

### Integration with Code Generation

#### 1. GlobalsLifter (`code_generator.rs`)

The `GlobalsLifter` transforms module-level variables that are referenced with global statements:

```rust
struct GlobalsLifter<'a> {
    global_info: &'a ModuleGlobalInfo,
}
```

**Transformation Process:**

1. **Identify Variables to Lift**: Variables that are both module-level and referenced in global statements
2. **Generate Lifted Names**: Create unique names like `__cribo_module_var` for lifted variables
3. **Create Declarations**: Generate top-level assignments for lifted variables
4. **Transform Functions**:
   - Add initialization statements mapping local names to lifted globals
   - Rewrite global statements to use lifted names
   - Transform variable references within functions

**Example Transformation:**

```python
# Original module code
result = "module_result"

def process():
    global result
    result = "processed"
    return result

# After lifting
__cribo_module_result = "module_result"

def process():
    result = __cribo_module_result  # Added initialization
    global __cribo_module_result
    __cribo_module_result = "processed"
    return __cribo_module_result
```

#### 2. globals() Transformation

When modules are wrapped, `globals()` calls need special handling to maintain correct namespace access:

```rust
fn transform_globals_in_expr(expr: &mut Expr) {
    match expr {
        Expr::Call(call_expr) => {
            if is_globals_call(call_expr) {
                // Transform globals() to module.__dict__
                *expr = create_module_dict_access();
            }
        } // ... recursive handling
    }
}
```

**Example:**

```python
# Original
data = globals().get('some_var')

# Transformed in wrapped module
data = module.__dict__.get('some_var')
```

### Symbol Resolution and Renaming

#### 1. Variable Name Conflict Resolution

The system handles conflicts between:

- Module-level variables across different modules
- Function/class names that might clash
- Variables referenced in global statements

**Resolution Strategy:**

1. **Detection**: Identify all symbols that could conflict
2. **Renaming**: Generate unique names using content hashing
3. **Mapping**: Maintain registry of original → renamed mappings
4. **Application**: Apply renames consistently throughout the AST

#### 2. Scoping Rules

The implementation respects Python's scoping rules:

```python
# Module level
Logger = "module"  # This gets renamed to Logger_1

def function():
    # Function level - shadows module level
    Logger = "local"  # This is NOT renamed
    return Logger  # Returns "local"

# After function, module level is accessible
print(Logger)  # This references the renamed Logger_1
```

**Key Insight**: Renames are only applied at module scope, not within function bodies where local variables shadow module-level ones.

## Implementation Details

### Module Analysis Flow

1. **Parse Module**: Use Ruff's parser to create AST
2. **Build Semantic Model**: Create bindings and scope information
3. **Analyze Global Usage**:
   ```rust
   let mut visitor = GlobalUsageVisitor::new(&mut global_info);
   visitor.visit_body(&module.body);
   ```
4. **Register Symbols**: Track all module-level definitions
5. **Apply Transformations**: Lift globals and resolve conflicts

### Test Framework Integration

The implementation includes comprehensive testing:

1. **Fixture Validation**: Tests run original Python code first to ensure it's valid
2. **Output Comparison**: Bundled output must match original execution
3. **Pretty Assertions**: Clear diffs when outputs don't match

```rust
// Run original fixture
let original_output = Command::new(&python_cmd)
    .arg(path)
    .current_dir(fixture_dir)
    .output()
    .expect("Failed to execute original fixture");

// Compare outputs
assert_eq!(
    original_stdout, 
    bundled_stdout,
    "\nBundled output differs from original for fixture '{}'",
    fixture_name
);
```

### Edge Cases Handled

1. **Nested Global Access**:
   ```python
   def outer():
       def inner():
           global x
           x = 10
   ```

2. **Complex Import Scenarios**:
   - Cross-module globals usage
   - Relative imports with global declarations
   - Module-level imports that shadow globals

3. **Special Function Handling**:
   - `globals()` calls in various contexts
   - Dynamic attribute access on module objects
   - Comprehensions and lambda functions

## Performance Considerations

1. **Single-Pass Analysis**: Module analysis happens once during bundling
2. **Efficient Data Structures**: Uses `FxIndexMap` and `FxIndexSet` for better performance
3. **Lazy Transformation**: Only transform when globals are actually used

## Future Enhancements

1. **F-string Transformation**: Handle f-strings that reference lifted globals
2. **Enhanced Diagnostics**: Better error messages for global conflicts
3. **Optimization**: Minimize number of lifted variables

## Configuration

The globals lifting feature is automatic and requires no configuration. It activates when:

- Module-level variables are defined
- Global statements reference those variables
- The bundler needs to maintain correct Python semantics

## Testing

The implementation is thoroughly tested with:

- Unit tests for individual components
- Integration tests with complex fixtures
- The `comprehensive_ast_rewrite` fixture demonstrating all features

Key test fixture that validates the implementation:

```
crates/cribo/tests/fixtures/bundling/comprehensive_ast_rewrite/
├── main.py                    # Entry point with conflicts
├── models/
│   ├── base.py               # Base model with relative imports
│   └── user.py               # User model with global usage
├── services/
│   └── auth/
│       └── manager.py        # Complex global patterns
└── core/
    ├── utils/
    │   └── helpers.py        # Utility functions
    └── database/
        └── connection.py     # Database module
```

This fixture tests:

- Multiple naming conflicts across modules
- Global variable lifting and access
- Cross-module imports and dependencies
- Complex scoping scenarios
- globals() function transformation
