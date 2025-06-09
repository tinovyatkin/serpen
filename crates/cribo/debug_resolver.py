import sys
import os

# Test what the resolver should find
test_dir = "tests/fixtures/stickytape_test_scripts/script_using_from_to_import_module"

print("Checking directory structure:")
print(f"greetings/ exists: {os.path.exists(os.path.join(test_dir, 'greetings'))}")
print(f"greetings/__init__.py exists: {os.path.exists(os.path.join(test_dir, 'greetings/__init__.py'))}")
print(f"greetings/greeting.py exists: {os.path.exists(os.path.join(test_dir, 'greetings/greeting.py'))}")

# This is NOT a package import - it's a module import from a directory
# The import `from greetings import greeting` means:
# - Look in directory `greetings/`
# - Find module `greeting.py`
# - Import it as `greeting`
