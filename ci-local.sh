#!/bin/bash
# ci-local.sh - Run the same checks that CI runs

set -e  # Exit on first error

CARGO="${CARGO:-cargo}"
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${YELLOW}=== Running CI checks locally ===${NC}\n"

# Format
echo -e "${YELLOW}[1/4] Formatting code...${NC}"
$CARGO fmt --all
echo -e "${GREEN}✓ Formatted${NC}\n"

# Clippy
echo -e "${YELLOW}[2/4] Running clippy...${NC}"
if $CARGO clippy --all-targets --features hosted -- -D warnings; then
    echo -e "${GREEN}✓ Clippy passed${NC}\n"
else
    echo -e "${RED}✗ Clippy failed${NC}"
    exit 1
fi

# Tests without features
echo -e "${YELLOW}[3/4] Running tests (no features)...${NC}"
if $CARGO test; then
    echo -e "${GREEN}✓ Tests passed (no features)${NC}\n"
else
    echo -e "${RED}✗ Tests failed (no features)${NC}"
    exit 1
fi

# Tests with hosted feature
echo -e "${YELLOW}[4/4] Running tests (--features hosted)...${NC}"
if $CARGO test --features hosted; then
    echo -e "${GREEN}✓ Tests passed (--features hosted)${NC}\n"
else
    echo -e "${RED}✗ Tests failed (--features hosted)${NC}"
    exit 1
fi

echo -e "${GREEN}=== All CI checks passed! ===${NC}"
