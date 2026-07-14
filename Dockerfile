# syntax=docker/dockerfile:1.7
# ============================================================
# Chimera CLI (NEXUS-OMEGA) — 多阶段 Dockerfile
#
# 产物:distroless 镜像,目标 < 100MB
# Binary: chimera-cli crate 产出名为 `aether`,镜像中重命名为 `chimera`
#         (保持对外统一命令名,契合 "Chimera CLI" 品牌)
# ============================================================

# ---------- Stage 1: Builder ----------
# WHY 使用 rust:1-bookworm-slim 而非 rust:1-slim:
# rust:1-slim 可能基于 Debian Trixie(13),而 runtime 阶段使用
# gcr.io/distroless/cc-debian12(Debian 12/Bookworm)。若 builder 的 glibc
# 版本高于 runtime,二进制会因 "GLIBC_x.xx not found" 启动失败。
# 固定 bookworm-slim 确保 builder 与 runtime 的 glibc 版本一致。
FROM rust:1-bookworm-slim AS builder

# 系统依赖:
# - pkg-config: 部分 crate 探测系统库时需要
# - libssl-dev: 备用(本项目用 rustls-tls,通常不需要,保留以防显式依赖)
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# 先复制 manifest 与 cargo 配置,利用 Docker 层缓存加速依赖编译
# (仅依赖变更时重编译 dependencies,改源码不触发)
COPY Cargo.toml Cargo.lock ./
COPY .cargo/config.toml ./.cargo/config.toml
COPY crates/ ./crates/

# Release 构建:仅构建 chimera-cli 的 binary(aether)
# workspace 级 [profile.release] 已配置 strip/lto/opt-level=z/panic=abort
RUN cargo build --release -p chimera-cli

# 打印 binary 的动态库依赖,便于排查 distroless 运行时缺少共享库的问题
RUN ldd target/release/aether 2>&1 || true

# ---------- Stage 2: Runtime (distroless) ----------
# WHY distroless/cc-debian12:
# - 无 shell、无包管理器,攻击面最小化(契合 #![forbid(unsafe_code)] 安全哲学)
# - 包含 glibc 动态链接器,可运行 GNU 链接的 Rust binary
# - 基础镜像约 20MB,加 binary 后总体积 < 100MB
# - 内置 nonroot 用户(UID 65532),无需 RUN adduser 创建(distroless 无 shell 也不支持 RUN)
FROM gcr.io/distroless/cc-debian12

# 运行时环境变量:
# - RUST_BACKTRACE=1:release 构建使用 panic=abort,默认无栈回溯;
#   开启 backtrace 让线上 panic 打印调用栈,便于事后定位(§10.5 P1 短板修复,2026-06-29)
# - RUST_LOG=info:默认 INFO 级别日志,兼顾可观测性与噪声控制
ENV RUST_BACKTRACE=1
ENV RUST_LOG=info

# 版本标签:CI 可通过 --build-arg VERSION=... 覆盖,默认与 workspace 版本一致
# WHY 用 ARG 而非硬编码:发布流水线需按 git tag 动态注入版本号,硬编码会导致版本漂移
ARG VERSION=1.7.0-omega

# OCI 标准 LABELS:镜像元数据,便于镜像仓库(GHCR/Docker Hub)索引与运维检索
# WHY 缺失 LABELS 的镜像在仓库中无法被搜索/过滤,违反 OCI 镜像规范的可发现性要求;
#      licenses 字段取自根 Cargo.toml 的 license = "Apache-2.0"(非 MIT,与项目实际一致)
LABEL org.opencontainers.image.title="Chimera CLI" \
      org.opencontainers.image.description="NEXUS-OMEGA AI Coding Agent — 全维稀疏架构的下一代编码代理" \
      org.opencontainers.image.source="https://github.com/Yoloccyt/Chimera-CLI-" \
      org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.version="${VERSION}"

# 构建产物 aether 复制为 chimera(对外统一命令名)
# --chown=nonroot:nonroot:文件归属 distroless 内置 nonroot 用户(UID/GID 65532)
# WHY 设置所有权:默认 COPY 以 root:root 归属,切换 nonroot 后将无法读取/执行 binary
COPY --from=builder --chown=nonroot:nonroot /app/target/release/aether /usr/local/bin/chimera

# 基础健康检查:验证 binary 可执行(无网络服务,仅检查进程存活与二进制完整性)
# WHY exec form(JSON 数组):distroless 无 shell,shell form 会因 /bin/sh 不存在而失败
# WHY chimera --version:CLI 工具非长驻进程,用 --version 验证 binary 可加载运行;
#      退出码 0 = healthy,非 0 = unhealthy,Docker 引擎自动判定(无需 || exit 1)
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
  CMD ["chimera", "--version"]

# 切换非 root 用户:distroless 内置 nonroot(UID 65532,无登录 shell)
# WHY 最小权限原则:默认以 root 运行会在容器逃逸时放大攻击面;
#      nonroot 是 distroless 标准用户,直接引用即可,无需 RUN 创建
USER nonroot:nonroot

# distroless 无 shell,必须用 exec form ENTRYPOINT
ENTRYPOINT ["chimera"]
