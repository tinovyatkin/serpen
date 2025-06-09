# Static Bundling System Design

## Overview

The static bundling feature in Cribo eliminates the use of runtime `exec()` calls by transforming Python modules into wrapper classes. This approach addresses compatibility issues with certain Python environments where `exec()` is restricted or problematic, particularly when dealing with hoisted stdlib imports.

## Problem Statement

The traditional bundling approach uses `exec()` to execute module code in isolated namespaces. This causes issues when:

- Hoisted standard library imports are not accessible within the exec'd code
- Security-restricted environments block or limit `exec()` usage
- Debugging becomes difficult due to dynamic code execution
- Performance overhead from runtime code compilation

## Solution Architecture

### Core Concept

Transform each Python module into a wrapper class where:

- Functions become static methods
- Module-level variables are stored in a `__cribo_vars` dictionary
- Complex initialization code goes into a `__cribo_init` classmethod
- Classes remain as nested classes within the wrapper

### Module Transformation

Each module `foo.bar` is transformed into a class `__cribo_module_foo_bar`:

```python
# Original module: foo/bar.py
VERSION = "1.0.0"
DEBUG = True

def helper(x):
    return x * 2

class Config:
    timeout = 30

# Complex initialization
if DEBUG:
    print("Debug mode enabled")
```

```python
# Transformed module
class __cribo_module_foo_bar:
    __cribo_vars = {
        'VERSION': "1.0.0",
        'DEBUG': True
    }
    
    @staticmethod
    def helper(x):
        return x * 2
    
    class Config:
        timeout = 30
    
    @classmethod
    def __cribo_init(cls):
        if DEBUG:
            print("Debug mode enabled")
```

### Module Facade System

After transformation, module facades are created to maintain Python's import semantics:

```python
import types

# Create module objects
foo = types.ModuleType('foo')
foo.bar = types.ModuleType('foo.bar')

# Copy attributes from wrapper to module
for __attr in dir(__cribo_module_foo_bar):
    if not __attr.startswith('_'):
        setattr(foo.bar, __attr, getattr(__cribo_module_foo_bar, __attr))

# Copy module variables
if hasattr(__cribo_module_foo_bar, '__cribo_vars'):
    for __k, __v in __cribo_module_foo_bar.__cribo_vars.items():
        setattr(foo.bar, __k, __v)

# Run initialization code
if hasattr(__cribo_module_foo_bar, '__cribo_init'):
    __cribo_module_foo_bar.__cribo_init()
```

### Import Rewriting

Import statements for bundled modules are handled specially:

1. **Simple imports** (`import foo`) - Removed entirely as module objects are pre-created
2. **From imports** (`from foo import bar`) - Converted to attribute access:
   ```python
   # Original
   from foo import bar

   # Transformed
   bar = getattr(foo, 'bar')
   ```

### Entry Module Handling

The entry module receives special treatment:

- Its code executes directly in the global scope (not wrapped in a class)
- This ensures the main script behaves as expected
- Import statements in the entry module are still rewritten to use bundled modules

## Implementation Details

### AST Transformation Rules

1. **Functions** → Static methods with `@staticmethod` decorator
2. **Classes** → Nested classes (unchanged)
3. **Simple assignments** → Stored in `__cribo_vars` if they don't reference other variables
4. **Complex assignments** → Placed in `__cribo_init` method
5. **Import statements** → Filtered out during transformation
6. **Control flow statements** → Placed in `__cribo_init` method
7. **Expression statements** → Placed in `__cribo_init` method

### Variable Reference Detection

The bundler analyzes assignment values to determine placement:

- Literals and constants → `__cribo_vars`
- Expressions with variable references → `__cribo_init`

This ensures variables are initialized in the correct order.

### Module Initialization Order

1. Transform all non-entry modules into wrapper classes
2. Create module facade objects (types.ModuleType)
3. Copy attributes from wrappers to module objects
4. Execute `__cribo_init` methods in dependency order
5. Execute entry module code directly

## Benefits

1. **No exec() calls** - Compatible with restricted environments
2. **Better debugging** - Stack traces show actual class/method names
3. **Static analysis friendly** - Tools can analyze the bundled code
4. **Performance** - No runtime code compilation overhead
5. **Security** - Reduced attack surface without dynamic execution

## Limitations

1. **Dynamic module manipulation** - Code that modifies `__dict__` directly may not work
2. **Module reload** - `importlib.reload()` won't work as expected
3. **Circular imports** - Handled but may require careful initialization ordering
4. **Bundle size** - Slightly larger due to wrapper infrastructure

## Configuration

Enable static bundling via:

```toml
# cribo.toml
[bundler]
static-bundling = true
```

Or via CLI:

```bash
cribo --entry main.py --output bundle.py --static-bundling
```

Or via environment variable:

```bash
CRIBO_STATIC_BUNDLING=true cribo --entry main.py --output bundle.py
```

## Future Enhancements

1. **Optimization passes** - Remove unnecessary wrapper overhead for simple modules
2. **Source maps** - Map bundled code back to original files for debugging
3. **Lazy module loading** - Defer module initialization until first access
4. **Tree shaking** - Remove unused module attributes from bundles
5. **Type preservation** - Maintain type hints in transformed code

## Testing Strategy

1. **Unit tests** - Test individual transformation rules
2. **Integration tests** - Test full bundling pipeline
3. **Compatibility tests** - Ensure feature parity with exec-based bundling
4. **Performance benchmarks** - Compare with traditional bundling
5. **Real-world projects** - Test with complex Python applications
