# Global Namespace Analysis for Python Bundling

## Executive Summary

This document analyzes Python's global namespace mechanics and how they interact with the ruff AST and semantic analysis crates, specifically in the context of the cribo bundler's module inlining challenges. The core issue is that when modules are transformed into wrapper functions, global statements that previously referred to module-level variables now fail because those variables exist in the wrapper's local scope, not the true global scope.

## Python Global Namespace Mechanics

### What Can Be Declared Global

In Python, the `global` keyword can be used to declare that a name refers to a variable in the global (module) namespace. The following can be declared global:

1. **Variables** (most common)
   ```python
   global x, y, z  # Multiple variables in one statement
   global result   # Single variable
   ```

2. **Any valid Python identifier**
   - Simple names: `global count`
   - Names with underscores: `global _private, __name__`
   - Names with numbers: `global var1, var2`

3. **Names that don't exist yet**
   ```python
   def func():
       global new_var  # Can declare before it exists
       new_var = 42    # Creates it in global scope
   ```

### Global Statement Semantics

The `global` statement affects name binding behavior:

1. **Without global**: Assignment creates a local variable
2. **With global**: Assignment modifies/creates a global variable

```python
x = 10  # Global

def without_global():
    x = 20  # Creates local x, shadows global
    
def with_global():
    global x
    x = 30  # Modifies global x
```

### Cross-Scope Interactions

Python's scoping rules with globals:

1. **Read access**: Can read globals without declaration
2. **Write access**: Requires global declaration
3. **Nested scopes**: Global affects the module scope, not enclosing function scopes

```python
x = 1

def outer():
    x = 2
    def inner():
        global x
        x = 3  # Modifies module-level x (1→3), not outer's x
    inner()
    print(x)  # Still 2
    
outer()
print(x)  # Now 3
```

## Ruff Python Semantic Analysis Capabilities

### What ruff_python_semantic Provides

1. **Binding Information**
   - `BindingKind::Global` - Identifies global declarations
   - `BindingFlags` - Tracks usage patterns
   - Scope hierarchy tracking

2. **Usage Tracking**
   - Can identify where names are referenced
   - Distinguishes between reads and writes
   - Tracks definition sites

3. **Scope Analysis**
   - `ScopeKind::Module`, `ScopeKind::Function`, `ScopeKind::Class`
   - Parent scope relationships
   - Symbol visibility analysis

### Limitations for Global Analysis

1. **Contextual Global Behavior**
   - Doesn't track runtime module transformation
   - Assumes standard Python module structure
   - No special handling for wrapper functions

2. **Cross-Module Global References**
   - Limited to single-module analysis
   - Can't track globals across module boundaries
   - No understanding of bundled module contexts

## Ruff Python AST Capabilities

### AST Representation of Globals

1. **Global Statement Node**
   ```rust
   pub struct StmtGlobal {
       pub names: Vec<Identifier>,
       pub range: TextRange,
   }
   ```

2. **Transformation Capabilities**
   - Can identify all global statements
   - Can modify/remove global declarations
   - Can rewrite variable accesses

3. **Pattern Matching**
   - Can find all uses of globally-declared names
   - Can distinguish assignment vs. reference
   - Can track through nested scopes

## The Bundling Challenge

### Current Problem

When transforming modules into wrapper functions:

```python
# Original module (models/base.py)
result = "base_result"

def initialize():
    global result
    result = f"initialized_{value}"
    return result
```

Becomes:

```python
def __cribo_init_module():
    result = "base_result"  # Now local to wrapper
    
    def initialize():
        global result  # Refers to non-existent global!
        result = f"initialized_{value}"  # NameError!
        return result
```

### Root Cause Analysis

1. **Scope Transformation**: Module-level becomes function-local
2. **Global References**: Still point to true global scope
3. **Variable Accessibility**: Module variables trapped in wrapper scope

## Technical Proposal: Complete Solution Architecture

### Solution 1: Global Proxy Pattern (Recommended)

Transform global statements to use a proxy mechanism:

```python
def __cribo_init_module():
    # Create module namespace
    __module_globals__ = {
        'result': "base_result"
    }
    
    def initialize():
        # Transform: global result
        # Into: nonlocal __module_globals__
        __module_globals__['result'] = f"initialized_{value}"
        return __module_globals__['result']
    
    # Expose for external access
    module.__globals__ = __module_globals__
```

**Implementation Steps:**

1. **AST Analysis Phase**
   - Identify all global statements
   - Track which names are declared global
   - Build a global usage map per module

2. **Transformation Phase**
   - Replace `global x` with nonlocal access to proxy dict
   - Transform `x = value` to `__module_globals__['x'] = value`
   - Transform reads to `__module_globals__['x']`

3. **Integration Phase**
   - Expose globals dict on module object
   - Handle cross-module global access
   - Maintain backward compatibility

### Solution 2: Module Globals Lifting

Lift module-level globals to true global scope with unique names:

```python
# Generated at module top-level
__cribo_base_result = "base_result"

def __cribo_init_module():
    def initialize():
        global __cribo_base_result
        __cribo_base_result = f"initialized_{value}"
        return __cribo_base_result
```

**Pros:**

- Maintains Python's global semantics
- No runtime overhead
- Simple implementation

**Cons:**

- Pollutes global namespace
- Potential naming conflicts
- Complex with many modules

### Solution 3: Semantic-Aware Rewriting (Most Complex)

Use semantic analysis to eliminate globals entirely:

1. **Analyze Global Usage**
   - Track all global declarations
   - Identify setter/getter patterns
   - Build dependency graph

2. **Transform to Explicit State**
   ```python
   class ModuleState:
       def __init__(self):
           self.result = "base_result"

   def __cribo_init_module():
       state = ModuleState()
       
       def initialize(state=state):
           state.result = f"initialized_{value}"
           return state.result
   ```

### Recommended Architecture

**Phase 1: Analysis Enhancement**

1. Extend `SemanticBundler` to track:
   ```rust
   pub struct GlobalUsageInfo {
       pub declared_globals: FxIndexSet<String>,
       pub global_reads: FxIndexMap<String, Vec<TextRange>>,
       pub global_writes: FxIndexMap<String, Vec<TextRange>>,
       pub cross_function_globals: FxIndexSet<String>,
   }
   ```

2. Build comprehensive global usage map during semantic analysis

**Phase 2: AST Transformation**

1. Implement `GlobalsTransformer`:
   ```rust
   impl GlobalsTransformer {
       fn transform_module(&mut self, module: &mut Module) {
           // 1. Create __module_globals__ dict
           // 2. Transform global statements
           // 3. Rewrite variable accesses
           // 4. Add initialization code
       }
   }
   ```

2. Integration points:
   - After module wrapping
   - Before import rewriting
   - Preserve semantic correctness

**Phase 3: Runtime Support**

1. Module proxy enhancements:
   ```python
   class CriboModule:
       def __init__(self, globals_dict):
           self.__globals__ = globals_dict
       
       def __getattr__(self, name):
           if name in self.__globals__:
               return self.__globals__[name]
           raise AttributeError(name)
   ```

## Implementation Roadmap

1. **Immediate (Fix current test)**
   - Implement basic global proxy for failing test
   - Document limitations

2. **Short-term (Full solution)**
   - Implement Solution 1 (Global Proxy Pattern)
   - Comprehensive test suite
   - Performance benchmarks

3. **Long-term (Optimization)**
   - Semantic-based optimization
   - Dead global elimination
   - Cross-module global analysis

## Conclusion

The global namespace problem in module bundling is solvable with proper architectural changes. The recommended Global Proxy Pattern provides the best balance of correctness, performance, and implementation complexity. Both ruff AST and semantic crates provide sufficient capabilities to implement this solution, though some extensions to semantic analysis would improve the robustness of the implementation.
