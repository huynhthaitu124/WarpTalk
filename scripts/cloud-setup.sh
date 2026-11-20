#!/usr/bin/env bash
# Setup script cho Claude Code cloud environment.
# Dán nội dung này vào ô "Setup script" của environment trên claude.ai/code,
# hoặc để environment gọi: bash scripts/cloud-setup.sh
set -euo pipefail

echo "WarpTalk cloud setup: blank project, nothing to install yet."

# Khi đã chọn stack, bỏ comment phần tương ứng:
# dotnet restore              # .NET
# npm ci                      # Node
# pip install -r requirements.txt  # Python
