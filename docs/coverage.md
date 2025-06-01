# Code Coverage

This project uses `cargo-llvm-cov` for code coverage analysis.

## Local Usage

The following cargo aliases are configured in `.cargo/config.toml`:

### Standard Coverage Commands

```bash
# Generate text coverage report (like Istanbul text reporter)
cargo coverage-text

# Generate HTML coverage report and open in browser
cargo coverage

# Generate LCOV format for CI/external tools
cargo coverage-lcov

# Generate JSON format
cargo coverage-json

# Clean coverage data
cargo coverage-clean
```

### Branch Coverage Commands (Requires Nightly Rust)

Branch coverage provides more detailed analysis by tracking which branches of conditional statements (`if`, `while`, `match` guards, `&&`, `||`) are taken during test execution.

```bash
# Generate HTML report with branch coverage and open in browser
cargo coverage-branch

# Generate text report with branch coverage
cargo coverage-branch-text

# Generate LCOV format with branch coverage
cargo coverage-branch-lcov
```

**Note**: Branch coverage is currently experimental and requires nightly Rust. To use branch coverage:

1. Install nightly toolchain: `rustup toolchain install nightly`
2. Use nightly for coverage: `cargo +nightly coverage-branch`

### Current Limitation

If you see **all zeros in the Branch Coverage column** in HTML reports, this is expected when using stable Rust. Branch coverage is only available with nightly Rust toolchain.

## Branch Coverage FAQ

### Q: Why do I see all zeros in the "Branch Coverage" column in HTML reports?

**A: This is expected behavior when using stable Rust.** Branch coverage is an experimental feature that only works with nightly Rust.

#### Understanding the Situation

**What You're Seeing:**

- **Line Coverage**: ✅ Shows actual coverage percentages (e.g., 95.2%)
- **Function Coverage**: ✅ Shows actual coverage percentages (e.g., 100%)
- **Branch Coverage**: ⚠️ Shows 0.00% for all files

**Why This Happens:**

1. **Stable Rust Limitation**: Branch coverage instrumentation is not available in stable Rust
2. **HTML Report Display**: The coverage report always shows the branch coverage column, even when no branch data is collected
3. **Expected Behavior**: Zeros in branch coverage with stable Rust is normal, not a configuration error

#### How Branch Coverage Works

Branch coverage tracks which branches of conditional statements are executed during tests:

**Supported Constructs (Nightly Only):**

- `if` and `else` conditions
- `while` loop conditions
- Match guards: `match x { Some(y) if y > 0 => ... }`
- Short-circuit operators: `&&` and `||`

**Not Yet Supported:**

- `if let` and `while let` statements
- `for` loops
- Regular `match` arms (without guards)
- Branches inside macro expansions

#### Solutions

**Option 1: Use Stable Rust (Current Setup)**

```bash
# Line and function coverage only
cargo coverage-text
cargo coverage  # HTML report with 0% branch coverage
```

**Result**: Excellent line/function coverage, zeros for branch coverage

**Option 2: Enable Branch Coverage with Nightly**

```bash
# Install nightly toolchain
rustup toolchain install nightly
rustup component add llvm-tools-preview --toolchain nightly

# Generate reports with branch coverage
cargo +nightly coverage-branch       # HTML with branch data
cargo +nightly coverage-branch-text  # Text with branch data
```

**Result**: Line, function, AND branch coverage data

**Option 3: Use Both (Recommended)**

```bash
# Regular development with stable
cargo coverage

# Detailed analysis with nightly (when needed)
cargo +nightly coverage-branch
```

#### Quick Setup for Branch Coverage

Run the provided script:

```bash
./scripts/setup-branch-coverage.sh
```

This script will:

- Install nightly Rust if needed
- Add required components
- Test branch coverage functionality
- Explain the differences

## Coverage Text Report

Run `cargo coverage-text` to see a per-file coverage table similar to Istanbul:

```
Filename                        Regions    Missed Regions     Cover   Functions  Missed Functions  Executed       Lines      Missed Lines     Cover
----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
/Volumes/.../bundler.rs             136              5     96.32%          20                 0    100.00%         309              20    93.53%
/Volumes/.../config.rs               23              0    100.00%           5                 0    100.00%          58               0   100.00%
...
```

## Current Project Status

✅ **Excellent Coverage**: The project has >90% line coverage across most files\
✅ **Stable Setup**: All coverage commands work perfectly with stable Rust\
✅ **Branch Coverage Ready**: Nightly support is configured and available\
⚠️ **Expected Zeros**: Branch coverage shows 0% with stable Rust (this is normal)

### Recommendation

**For most development**: Continue using stable Rust with the current coverage setup. The zeros in branch coverage are expected and don't indicate any problems.

**For detailed analysis**: Occasionally use `cargo +nightly coverage-branch` to see which conditional branches are being tested.

The current coverage setup is working perfectly - the zero branch coverage is a display artifact, not a coverage gap!

## CI Integration

Coverage is automatically generated on every push and PR via GitHub Actions:

- **Branch coverage** is enabled in CI using nightly Rust for comprehensive analysis
- Coverage reports (with branch data) are uploaded to Codecov
- LCOV files with branch information are saved as artifacts
- Coverage changes are commented on PRs automatically
- GitHub status checks show coverage status

### Codecov Setup

To enable Codecov integration:

1. Go to [Codecov](https://codecov.io) and sign up with your GitHub account
2. Add your repository to Codecov
3. Copy the repository upload token
4. Add the token as `CODECOV_TOKEN` in your GitHub repository secrets:
   - Go to Settings → Secrets and variables → Actions
   - Click "New repository secret"
   - Name: `CODECOV_TOKEN`
   - Value: your Codecov upload token

### Codecov Configuration

The `.codecov.yml` file configures:

- **Coverage targets**: 80% project and patch coverage
- **PR comments**: Automatic coverage change reports
- **GitHub checks**: Status annotations on commits
- **File filtering**: Excludes test files, docs, and build artifacts

## Setup Requirements

The coverage system requires:

1. `cargo-llvm-cov` to be installed (`cargo install cargo-llvm-cov`)
2. `llvm-tools-preview` component (`rustup component add llvm-tools-preview`)

These are automatically installed in the CI environment.

## Files Generated

Coverage reports are saved to:

- `target/llvm-cov/html/` - HTML reports
- `target/llvm-cov/lcov.info` - LCOV format for CI
- `target/llvm-cov/coverage.json` - JSON format

These directories are automatically ignored by git.
