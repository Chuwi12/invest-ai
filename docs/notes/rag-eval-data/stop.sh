#!/bin/bash
R=$HOME/.cache/invest-ai/rag-eval
p=$(cat $R/$1.pid 2>/dev/null); [ -n "$p" ] && kill "$p" 2>/dev/null; rm -f $R/$1.pid; echo "stopped $1 ($p)"
