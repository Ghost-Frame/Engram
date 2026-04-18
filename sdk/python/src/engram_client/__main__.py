"""
Entry point for `python -m engram_client`.

Prints version and available symbols, confirming the package imports cleanly.
"""

from . import __version__

print(f"engram-client {__version__}")
print("Available: EngramClient, AsyncEngramClient, EngramError, Memory, SearchResult, ...")
print("See README.md for usage.")
