"""
Entry point for `python -m kleos_client`.

Prints version and available symbols, confirming the package imports cleanly.
"""

from . import __version__

print(f"kleos-client {__version__}")
print("Available: KleosClient, AsyncKleosClient, KleosError, Memory, SearchResult, ...")
print("See README.md for usage.")
