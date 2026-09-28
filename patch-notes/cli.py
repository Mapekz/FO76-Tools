#!/usr/bin/env python3
"""Run the pipeline from anywhere: `python3 patch-notes/cli.py <verb> [args]`
is `python3 -m pn <verb> [args]` with this directory on the import path."""

import sys

from pn.__main__ import main

sys.exit(main())
