# 📋 Galaxy-Ops Project Requirements & Constraints

**版本：** 1.0
**最后更新：** 2025年1月

---

## 🎯 项目概述

**Galaxy-Ops** 是一个现代化的系统运维工具，采用 Rust 开发，专注于多平台支持和原生性能。本项目通过优化 CI/CD 策略，实现了基于 GitHub Actions 原生环境的极简但功能完整的开发工作流。

---

## 📊 核心需求

---

###  平台与技术需求 ✅

| 需求项 | 具体要求 | 状态 | 实现方案 |
|--------|----------|------|----------|
| **平台支持** | Linux + macOS (无 Windows) | ✅ 完成 | Linux (x86_64/ARM64) + macOS ARM64 |
| **Linux 支持** | 统一 musl 静态链接（不再出 glibc 制品） | ✅ 完成 | x86_64-unknown-linux-musl + aarch64-unknown-linux-musl |
| **macOS 支持** | 仅 Apple Silicon (ARM64) | ✅ 完成 | aarch64-apple-darwin |
| **原生环境** | GitHub Actions 原生工具链 | ✅ 完成 | 无 Cross/Docker，原生 musl-gcc |
| **静态链接** | musl 二进制完整验证 | ✅ 完成 | `file` 验证 "statically linked" |
| **制品命名** | 固定平台矩阵，资产名可稳定解析 | ✅ 完成 | `<repo>-<version>-<target-triple>.tar.gz`，包内顶层目录同名 |
