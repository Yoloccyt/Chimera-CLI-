# syntax=docker/dockerfile:1.7
# ============================================================
# Chimera CLI (NEXUS-OMEGA) — 多阶段 Dockerfile
#
# 产物:distroless 镜像,目标 < 100MB
# Binary: chimera-cli crate 产出名为 `aether`,镜像中重命名为 `chimera`
#         (保持对外统一命令名,契合 "Chimera CLI" 品牌)
# ============================================================

# ---------- Stage 1: Builder ----------
FROM rust:1.82-slim AS builder

# 系统依赖:
# - pkg-config: 部分 crate 探测系统库时需要
# - libssl-dev: 备用(本项目用 rustls-tls,通常不需要,保留以防显式依赖)
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# 先复制 manifest,利用 Docker 层缓存加速依赖编译
# (仅依赖变更时重编译 dependencies,改源码不触发)
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/

# Release 构建:仅构建 chimera-cli 的 binary(aether)
# workspace 级 [profile.release] 已配置 strip/lto/opt-level=z/panic=abort
RUN cargo build --release -p chimera-cli

# ---------- Stage 2: Runtime (distroless) ----------
# WHY distroless/cc-debian12:
# - 无 shell、无包管理器,攻击面最小化(契合 #![forbid(unsafe_code)] 安全哲学)
# - 包含 glibc 动态链接器,可运行 GNU 链接的 Rust binary
# - 基础镜像约 20MB,加 binary 后总体积 < 100MB
FROM gcr.io/distroless/cc-debian12

# 构建产物 aether 复制为 chimera(对外统一命令名)
COPY --from=builder /app/target/release/aether /usr/local/bin/chimera

# distroless 无 shell,必须用 exec form ENTRYPOINT
ENTRYPOINT ["chimera"]
