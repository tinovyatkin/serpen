from mypackage import config

# This should use the config object that was imported in __init__.py
DEBUG_MODE = config.DEBUG
print(f"Debug mode: {DEBUG_MODE}")
print(f"Log level: {config.LOG_LEVEL}")
print("Test completed successfully")
