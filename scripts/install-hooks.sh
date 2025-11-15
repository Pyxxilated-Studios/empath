#!/usr/bin/env bash
# Install Git pre-commit hooks for empath MTA
# This script sets up pre-commit hooks that run formatting and linting checks

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Get the repository root directory
REPO_ROOT=$(git rev-parse --show-toplevel 2>/dev/null)

if [ -z "$REPO_ROOT" ]; then
    echo -e "${RED}Error: Not in a git repository${NC}"
    exit 1
fi

HOOKS_DIR="$REPO_ROOT/.git/hooks"
HOOK_FILE="$HOOKS_DIR/pre-commit"

# Create hooks directory if it doesn't exist
mkdir -p "$HOOKS_DIR"

# Create the pre-commit hook
cat > "$HOOK_FILE" << 'EOF'
#!/usr/bin/env bash
# Pre-commit hook for empath MTA
# Runs format check and clippy before allowing commit

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${YELLOW}Running pre-commit checks...${NC}"
echo ""

# Check 1: Format check
echo -e "${YELLOW}[1/2] Checking code formatting...${NC}"
if ! cargo fmt --check; then
    echo ""
    echo -e "${RED}✗ Format check failed!${NC}"
    echo -e "${YELLOW}Run 'cargo fmt' to fix formatting issues${NC}"
    echo ""
    echo -e "To bypass this check (not recommended): git commit --no-verify"
    exit 1
fi
echo -e "${GREEN}✓ Format check passed${NC}"
echo ""

# Check 2: Clippy
echo -e "${YELLOW}[2/2] Running clippy...${NC}"
if ! cargo clippy --all-targets --all-features -- -D warnings; then
    echo ""
    echo -e "${RED}✗ Clippy found issues!${NC}"
    echo -e "${YELLOW}Fix the issues above before committing${NC}"
    echo ""
    echo -e "To bypass this check (not recommended): git commit --no-verify"
    exit 1
fi
echo -e "${GREEN}✓ Clippy check passed${NC}"
echo ""

echo -e "${GREEN}All pre-commit checks passed!${NC}"
echo ""
EOF

# Make the hook executable
chmod +x "$HOOK_FILE"

echo -e "${GREEN}✓ Pre-commit hook installed successfully!${NC}"
echo ""
echo "The following checks will run before each commit:"
echo "  1. Code formatting (cargo fmt --check)"
echo "  2. Linting (cargo clippy)"
echo ""
echo -e "${YELLOW}Note:${NC} You can bypass the hook with: git commit --no-verify"
echo "      (Only use in emergencies!)"
echo ""
