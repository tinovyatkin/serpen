#!/usr/bin/env bash
# Enhanced benchmark script with Bencher.dev integration

set -euo pipefail

# Load environment variables if .env exists
if [ -f .env ]; then
    export $(grep -v '^#' .env | xargs)
fi

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
PURPLE='\033[0;35m'
NC='\033[0m' # No Color

# Default values
BENCHER_PROJECT="${BENCHER_PROJECT:-serpen}"
BENCHER_TESTBED="${BENCHER_TESTBED:-local}"
BENCHER_ADAPTER="${BENCHER_ADAPTER:-json}"

echo -e "${PURPLE}🐰 Serpen Benchmarks with Bencher.dev${NC}"
echo "========================================"

# Check if we're in the right directory
if [[ ! -f "Cargo.toml" ]]; then
    echo -e "${RED}Error: Must run from project root directory${NC}"
    exit 1
fi

# Check if bencher CLI is installed
if ! command -v bencher &> /dev/null; then
    echo -e "${YELLOW}Bencher CLI not found. Installing...${NC}"
    cargo install bencher_cli
fi

# Check for API token
if [ -z "${BENCHER_API_TOKEN:-}" ]; then
    echo -e "${RED}Error: BENCHER_API_TOKEN not set${NC}"
    echo "Set it in .env file or export BENCHER_API_TOKEN=your-token"
    echo ""
    echo "Get your token from: https://bencher.dev/console/settings/tokens"
    exit 1
fi

# Build in release mode first
echo -e "${YELLOW}Building in release mode...${NC}"
cargo build --release

# Run benchmarks with Bencher
echo -e "${GREEN}Running benchmarks with Bencher.dev...${NC}"
echo ""

# Run Criterion benchmarks with Bencher
echo -e "${BLUE}Running Criterion benchmarks...${NC}"
bencher run \
    --project "$BENCHER_PROJECT" \
    --token "$BENCHER_API_TOKEN" \
    --testbed "$BENCHER_TESTBED" \
    --adapter "$BENCHER_ADAPTER" \
    --iter 1 \
    "cargo bench --bench bundling -- --output-format bencher"

# Run CLI benchmarks with hyperfine
if command -v hyperfine &> /dev/null; then
    echo ""
    echo -e "${BLUE}Running CLI performance benchmarks...${NC}"
    
    # Create test project
    mkdir -p test_project/utils test_project/models
    echo "from utils.helpers import helper" > test_project/main.py
    echo "def helper(): return 'test'" > test_project/utils/helpers.py
    echo "from models.user import User" >> test_project/main.py
    echo "class User: pass" > test_project/models/user.py
    
    # Run hyperfine benchmarks with Bencher
    bencher run \
        --project "$BENCHER_PROJECT" \
        --token "$BENCHER_API_TOKEN" \
        --testbed "$BENCHER_TESTBED" \
        --adapter "json" \
        --iter 1 \
        "hyperfine --export-json /dev/stdout --runs 5 \
            --setup 'rm -f test_project/bundle.py' \
            'target/release/serpen --entry test_project/main.py --output test_project/bundle.py' \
            --cleanup 'rm -f test_project/bundle.py'"
    
    # Cleanup
    rm -rf test_project
else
    echo -e "${YELLOW}hyperfine not found. Skipping CLI benchmarks.${NC}"
    echo "Install with: cargo install hyperfine"
fi

echo ""
echo -e "${PURPLE}View results at:${NC}"
echo "  https://bencher.dev/console/projects/${BENCHER_PROJECT}/perf"
echo ""
echo -e "${GREEN}✓ Benchmarks completed and sent to Bencher.dev!${NC}"