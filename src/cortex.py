#!/usr/bin/env python3
"""Cortex MCP stdio entry point."""
from cortex_runtime.tempdirs import prefer_disk_tmp
from cortex_runtime.server import main

if __name__ == '__main__':
    prefer_disk_tmp()
    main()
