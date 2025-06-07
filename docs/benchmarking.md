# Serpen Benchmarking Guide

This document describes the benchmarking infrastructure for Serpen, designed to track performance regressions and ensure consistent bundling performance.

## Overview

Serpen uses [Criterion.rs](https://github.com/bheisler/criterion.rs) for micro-benchmarking with three layers of integration:

1. **Local Development**: Quick performance checks during development
2. **PR Comments**: Automatic performance comparison on pull requests
3. **Historical Tracking**: Long-term performance trends (future enhancement)

## Local Benchmarking

### Quick Start

```bash
# Run all benchmarks
cargo bench

# Run with our helper script (recommended)
./scripts/bench.sh

# Save a baseline
./scripts/bench.sh --save-baseline main

# Compare against baseline
./scripts/bench.sh --baseline main

# Open HTML report
./scripts/bench.sh --open
```

### Cargo Aliases

We provide convenient aliases for common operations:

```bash
cargo bench-local     # Run benchmarks
cargo bench-save      # Save baseline as 'main'
cargo bench-compare   # Compare against 'main' baseline
```

## Benchmarked Operations

### Core Operations (Critical Path)

1. **bundle_simple_project**: End-to-end bundling of a multi-module project
2. **parse_python_ast**: Python AST parsing performance
3. **resolve_module_path**: Module resolution speed

### Supporting Operations

4. **extract_imports**: Import statement extraction
5. **build_dependency_graph**: Dependency graph construction

## Performance Targets

### Acceptable Performance

- **Individual benchmarks**: ≤3% regression (within noise margin)
- **Overall bundling**: ≤1% regression
- **With justification**: Up to 5% for significant features

### Unacceptable Regressions

- **>5%** for any core operation without justification
- **>10%** for any benchmark (indicates algorithmic issue)
- **Any regression** in AST parsing (critical path)

## CI Integration

### Pull Request Comments

Every PR automatically receives benchmark comparison comments:

```
## Benchmark Results

### bundle_simple_project
- **main**: 3,412 ns/iter (± 185)
- **PR**: 2,987 ns/iter (± 147)
- **Change**: -12.45% 🎉 (Performance improved)

### parse_python_ast
- **main**: 45,123 ns/iter (± 2,341)
- **PR**: 46,789 ns/iter (± 2,156)
- **Change**: +3.69% ⚠️ (Slight regression)
```

### Workflow Files

- **`.github/workflows/benchmarks.yml`**: PR benchmark comparisons
- **`.github/workflows/benchmark-dashboard.yml`**: Historical tracking (future)

## Writing New Benchmarks

Add benchmarks to `crates/serpen/benches/bundling.rs`:

```rust
use criterion::{Criterion, black_box, criterion_group, criterion_main};

fn benchmark_new_feature(c: &mut Criterion) {
    c.bench_function("new_feature", |b| {
        b.iter(|| {
            // Your code here
            let result = expensive_operation(black_box(input));
            black_box(result); // Prevent optimization
        });
    });
}

// Add to the criterion_group!
criterion_group!(
    benches,
    benchmark_bundling,
    benchmark_new_feature, // Add here
                           // ... other benchmarks
);
```

## Performance Debugging

### When Benchmarks Regress

1. **Identify the regression**:
   ```bash
   cargo bench-compare
   ```

2. **Profile the code**:
   ```bash
   cargo install flamegraph
   cargo flamegraph --bench bundling
   ```

3. **Focus on hotspots**:
   - Check algorithmic complexity
   - Look for unnecessary allocations
   - Consider data structure choices

4. **Verify improvements**:
   ```bash
   cargo bench-compare
   ```

### Common Optimization Patterns

1. **Avoid repeated allocations**: Use `with_capacity()` for collections
2. **Minimize cloning**: Use references where possible
3. **Cache computations**: Store results of expensive operations
4. **Use efficient data structures**: `IndexMap` over `HashMap` for determinism

## Future Enhancements

### Bencher.dev Integration

We plan to integrate [Bencher.dev](https://bencher.dev) for:

- Historical performance tracking
- Statistical regression detection
- Cross-platform benchmarking
- Python benchmark integration

### Additional Metrics

Future benchmarks will track:

- Memory usage
- Binary size
- Python execution performance of bundled code
- Large project bundling (real-world scenarios)

## Maintenance

### Updating Baselines

After significant performance improvements:

```bash
# On main branch after merge
git checkout main
git pull
./scripts/bench.sh --save-baseline main
```

### Benchmark Stability

- Run benchmarks on quiet systems
- Close unnecessary applications
- Use consistent CPU governor settings
- Consider using `nice -n -20` for priority

## Troubleshooting

### "No baseline found"

Save a baseline first:

```bash
cargo bench-save
```

### Noisy Results

- Increase sample size in Criterion
- Run with higher priority: `nice -n -20 cargo bench`
- Check for background processes

### CI Failures

Check the workflow logs:

```bash
gh run list --workflow=benchmarks.yml
gh run view <run-id>
```
