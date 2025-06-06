#!/usr/bin/env python3
"""Test late future imports - these should trigger F404 rule"""

# Regular imports first
import os
import sys

# This future import is late - should trigger F404
from __future__ import annotations


def main():
    """Main function with future annotations"""
    name: str = "test"
    print(f"Hello {name}")
    print(f"OS: {os.name}")
    print(f"Python version: {sys.version[:5]}")


if __name__ == "__main__":
    main()
