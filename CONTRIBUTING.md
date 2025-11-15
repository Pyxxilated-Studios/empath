# Contributing to Empath MTA

Thank you for your interest in contributing to Empath! This document provides guidelines and best practices for contributing to the project.

## Table of Contents

- [Getting Started](#getting-started)
- [How to Contribute](#how-to-contribute)
- [Code Style and Standards](#code-style-and-standards)
- [Testing Requirements](#testing-requirements)
- [Pull Request Process](#pull-request-process)
- [Commit Message Guidelines](#commit-message-guidelines)
- [Code Review Process](#code-review-process)
- [Community Guidelines](#community-guidelines)
- [Getting Help](#getting-help)

---

## Getting Started

**New to the project?** Start here:

1. **Read the documentation:**
   - [README.md](README.md) - Project overview
   - [docs/ONBOARDING.md](docs/ONBOARDING.md) - Complete onboarding guide (<30 min)
   - [CLAUDE.md](CLAUDE.md) - Architecture and coding standards

2. **Set up your environment:**
   ```bash
   git clone https://github.com/Pyxxilated-Studios/empath.git
   cd empath
   just setup    # Installs all dev tools
   just doctor   # Verify environment health
   just build    # Build the project
   just test     # Run tests
   ```

3. **Verify everything works:**
   ```bash
   just ci       # Run full CI check (lint + fmt + test)
   ```

**Resources:**
- [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md) - Common issues and solutions
- [TODO.md](TODO.md) - Current roadmap and available tasks

---

## How to Contribute

We welcome all types of contributions:

### 1. Code Contributions

**Good First Issues:**
- Add tests for uncovered code
- Fix clippy warnings
- Improve error messages
- Add documentation examples

**More Involved:**
- Implement features from [TODO.md](TODO.md)
- Fix bugs reported in issues
- Performance improvements
- New protocol implementations

### 2. Documentation

- Improve existing documentation
- Add examples and tutorials
- Fix typos and clarify confusing sections
- Translate documentation

### 3. Testing

- Add unit tests
- Add integration tests
- Add benchmark tests
- Report bugs with reproduction steps

### 4. Design & UX

- Improve error messages
- Design architecture diagrams
- Improve CLI output formatting

### 5. Community

- Help others in discussions
- Review pull requests
- Report bugs
- Suggest features

---

## Code Style and Standards

Empath uses **strict clippy lints** (all + pedantic + nursery). All code must pass these checks.

### Clippy Requirements

```bash
# Must pass with zero warnings
just lint
# Or:
cargo clippy --all-targets --all-features
```

**Key requirements:**
- Functions must be under 100 lines (extract helpers if needed)
- No wildcard imports (use explicit imports)
- Use `try_from()` instead of `as` for potentially truncating casts
- Add `# Panics` sections for functions that may panic
- Use byte string literals `b"..."` instead of `"...".as_bytes()`
- Avoid holding locks longer than necessary
- Document code items in backticks (e.g., `` `PostDot` state ``)

See [CLAUDE.md](CLAUDE.md) "Clippy Configuration" for complete details.

### Code Formatting

```bash
# Format all code
just fmt

# Check formatting without modifying
just fmt-check
```

**Format standards:**
- Rust files: 4 spaces, 100 char line length
- TOML/YAML: 2 spaces
- Use `.editorconfig` for automatic editor configuration

### Naming Conventions

- **Types**: `PascalCase`
- **Functions/variables**: `snake_case`
- **Constants**: `SCREAMING_SNAKE_CASE`
- **Lifetimes**: `'a`, `'b`, etc. (short and descriptive)

### Documentation

All public items must have documentation:

```rust
/// Brief description of what this function does.
///
/// More detailed explanation if needed.
///
/// # Arguments
///
/// * `arg1` - Description of arg1
/// * `arg2` - Description of arg2
///
/// # Returns
///
/// Description of return value
///
/// # Errors
///
/// When this function returns an error and why
///
/// # Panics
///
/// When this function panics (if applicable)
///
/// # Examples
///
/// ```
/// use empath::example;
/// let result = example("test");
/// assert_eq!(result, expected);
/// ```
pub fn example(arg1: &str, arg2: i32) -> Result<String> {
    // Implementation
}
```

---

## Testing Requirements

All code changes must include tests.

### Running Tests

```bash
# Run all tests
just test

# Run specific test
just test-one test_name

# Run tests for specific crate
just test-crate empath-smtp

# Run with coverage (if available)
cargo tarpaulin --out Html
```

### Test Coverage Requirements

- **New features**: 100% coverage of new code
- **Bug fixes**: Test that reproduces the bug + fix verification
- **Refactoring**: Existing tests must still pass

### Test Types

**Unit Tests:**
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_feature() {
        // Arrange
        let input = "test";

        // Act
        let result = function(input);

        // Assert
        assert_eq!(result, expected);
    }
}
```

**Integration Tests:**
- Use `MemoryBackedSpool` for spool operations
- Use `wait_for_count()` for async verification
- Clean up resources in test teardown

**Benchmark Tests:**
```bash
# Run benchmarks
just bench

# Save baseline
just bench-baseline-save my-feature

# Compare against baseline
just bench-baseline-compare my-feature
```

### Async Tests

```rust
#[tokio::test]
async fn test_async_feature() {
    // Test implementation
}
```

---

## Pull Request Process

### 1. Before You Start

- Check [TODO.md](TODO.md) for available tasks
- Comment on the issue you want to work on
- For large changes, open a discussion first

### 2. Create Your Branch

```bash
git checkout -b feature/descriptive-name
# Or:
git checkout -b fix/bug-description
```

**Branch naming:**
- `feature/` - New features
- `fix/` - Bug fixes
- `docs/` - Documentation changes
- `refactor/` - Code refactoring
- `test/` - Test additions/fixes

### 3. Make Your Changes

```bash
# Make changes
# ...

# Run development checks
just dev    # fmt + lint + test

# Run full CI locally
just ci     # lint + fmt-check + test
```

**Git pre-commit hooks** automatically run `fmt-check` and `lint` before each commit.

### 4. Commit Your Changes

Follow [Conventional Commits](https://www.conventionalcommits.org/):

```bash
git commit -m "feat: add new feature description"
git commit -m "fix: resolve bug in component"
git commit -m "docs: improve documentation for feature"
git commit -m "refactor: extract helper method"
git commit -m "test: add tests for edge case"
```

See [Commit Message Guidelines](#commit-message-guidelines) below.

### 5. Push and Create PR

```bash
git push -u origin feature/descriptive-name
```

Create pull request on GitHub with:
- **Clear title** following conventional commits format
- **Description** of what changed and why
- **Test plan** - how you verified the changes
- **Breaking changes** - if any, clearly marked
- **Related issues** - link to related issues

### 6. PR Template

```markdown
## Summary
Brief description of changes

## Changes
- Change 1
- Change 2

## Test Plan
1. Step 1
2. Step 2
3. Expected result

## Breaking Changes
- [ ] This PR includes breaking changes
- If yes, describe migration path

## Checklist
- [ ] Tests added/updated
- [ ] Documentation updated
- [ ] CHANGELOG.md updated (if user-facing)
- [ ] All CI checks passing
```

---

## Commit Message Guidelines

We follow [Conventional Commits](https://www.conventionalcommits.org/) specification.

### Format

```
<type>(<scope>): <description>

[optional body]

[optional footer(s)]
```

### Types

- **feat**: New feature
- **fix**: Bug fix
- **docs**: Documentation changes
- **refactor**: Code refactoring (no functional changes)
- **test**: Adding or updating tests
- **perf**: Performance improvements
- **ci**: CI/CD changes
- **build**: Build system changes
- **chore**: Maintenance tasks

### Scopes (Optional)

- `smtp` - SMTP protocol changes
- `delivery` - Delivery system changes
- `spool` - Spool system changes
- `ffi` - FFI/module system changes
- `config` - Configuration changes
- `docs` - Documentation
- `tests` - Tests

### Examples

```bash
# Simple feature
feat: add benchmark baseline tracking

# Feature with scope
feat(smtp): implement STARTTLS extension

# Bug fix with scope
fix(delivery): prevent duplicate delivery on retry

# Breaking change
feat(config)!: change config format to TOML

BREAKING CHANGE: Configuration format changed from RON to TOML.
Migration guide: ...

# Multiple paragraphs
refactor(spool): extract file operations to separate module

This refactoring improves testability by separating file I/O
from business logic.

The FileBackedSpool now uses a FileOperations trait that can
be mocked in tests.
```

### Rules

- Use present tense ("add feature" not "added feature")
- Use imperative mood ("move cursor to..." not "moves cursor to...")
- Don't capitalize first letter
- No period at the end
- Keep first line under 72 characters
- Explain *what* and *why*, not *how* (code shows how)

---

## Code Review Process

### For Contributors

**When your PR is under review:**

1. **Respond to feedback promptly**
   - Address comments or explain why you disagree
   - Ask for clarification if needed

2. **Make requested changes**
   ```bash
   # Make changes
   git add .
   git commit -m "fix: address review feedback"
   git push
   ```

3. **Request re-review**
   - After addressing all comments
   - Explain what you changed

### For Reviewers

**When reviewing PRs:**

1. **Be constructive and respectful**
   - Explain *why* changes are needed
   - Provide examples of better approaches
   - Acknowledge good work

2. **Check for:**
   - Code correctness
   - Test coverage
   - Documentation
   - Performance implications
   - Security issues
   - API design
   - Backwards compatibility

3. **Review types:**
   - **Approve**: Ready to merge
   - **Request changes**: Issues must be fixed
   - **Comment**: Suggestions, not blocking

---

## Community Guidelines

### Code of Conduct

- **Be respectful** - Treat everyone with respect
- **Be constructive** - Focus on improving the project
- **Be collaborative** - Work together towards common goals
- **Be patient** - Not everyone has the same experience level

### Communication

- **GitHub Issues** - Bug reports, feature requests
- **Pull Requests** - Code changes and discussions
- **Discussions** - General questions and ideas

### Reporting Issues

**Bug Reports:**
```markdown
**Description**
Clear description of the bug

**Steps to Reproduce**
1. Step 1
2. Step 2
3. See error

**Expected Behavior**
What should happen

**Actual Behavior**
What actually happens

**Environment**
- OS: Linux/macOS/Windows
- Rust version: `rustc --version`
- Empath version: `git rev-parse HEAD`

**Logs**
```
relevant logs here
```
```

**Feature Requests:**
```markdown
**Problem**
What problem does this solve?

**Proposed Solution**
How would you solve it?

**Alternatives**
What alternatives did you consider?

**Additional Context**
Any other relevant information
```

---

## Getting Help

**Before asking for help:**
1. Check [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md)
2. Search existing issues
3. Read relevant documentation in [CLAUDE.md](CLAUDE.md)
4. Run `just doctor` to diagnose environment issues

**Where to ask:**
- **Simple questions**: GitHub Discussions
- **Bug reports**: GitHub Issues
- **Feature proposals**: GitHub Discussions first, then issue

**What to include:**
- What you're trying to do
- What you've tried
- Relevant logs/errors
- Environment information

---

## Recognition

Contributors are recognized in:
- Git commit history
- Release notes
- CHANGELOG.md (for significant contributions)

**Thank you for contributing to Empath! 🚀**

---

## Quick Reference

**Development workflow:**
```bash
just dev              # fmt + lint + test
just ci               # Full CI check
just fix              # Auto-fix lint + format
```

**Before committing:**
```bash
just ci               # Must pass
```

**Before pushing:**
```bash
git push              # Pre-commit hook runs automatically
```

**Creating PR:**
1. Clear title with conventional commits format
2. Description of changes
3. Test plan
4. Link to related issues

**Questions?** See [Getting Help](#getting-help)
