# 网络代理配置方案指南

> **适用场景**: 企业网络环境，所有 443 端口出站被阻断
> **影响范围**: GitHub (winget, git clone, cargo install, cargo audit), Docker Hub, crates.io, gcr.io, Docker gcr.io/distroless/cc-debian12
> **项目工具链**: GNU 工具链 (`stable-x86_64-pc-windows-gnu`)，位于 `D:\Chimera CLI\.toolchain\`
> **CI/CD**: GitHub Actions (release.yml, fuzz.yml, audit.yml)

---

## 目录

1. [环境诊断](#1-环境诊断)
2. [方案 A — HTTP/HTTPS 代理（推荐）](#2-方案-a--httphttps-代理推荐)
3. [方案 B — SSH 隧道（备选）](#3-方案-b--ssh-隧道备选)
4. [方案 C — 离线操作（无奈之选）](#4-方案-c--离线操作无奈之选)
5. [验证步骤](#5-验证步骤)
6. [故障排查](#6-故障排查)
7. [CI/CD 特殊注意事项](#7-cicd-特殊注意事项)

---

## 1. 环境诊断

### 1.1 快速连通性测试

在 PowerShell 中执行以下命令，判断当前网络限制范围：

```powershell
# 测试 GitHub
Test-NetConnection github.com -Port 443
# 预期失败: TcpTestSucceeded = False

# 测试 crates.io
Test-NetConnection crates.io -Port 443

# 测试 gcr.io
Test-NetConnection gcr.io -Port 443

# 测试 Docker Hub
Test-NetConnection docker.io -Port 443

# 测试原始 TCP 连通性（排除 DNS 问题）
Test-NetConnection 140.82.113.4 -Port 443   # GitHub IP 示例
```

### 1.2 实际请求验证

```powershell
# 测试 HTTPS 请求
Invoke-WebRequest -Uri https://github.com -UseBasicParsing -TimeoutSec 10
# 预期: 连接超时或 403/407

# 测试 cargo 源
Invoke-WebRequest -Uri https://static.crates.io/crates.tar.gz -UseBasicParsing -TimeoutSec 10
```

### 1.3 诊断结果判断

| 现象 | 可能原因 | 适用方案 |
|------|----------|---------|
| 所有 443 端口超时 | 防火墙出站规则阻断 | 方案 A（HTTP 代理）或方案 B（SSH 隧道） |
| 返回 407 认证失败 | 已识别到代理但未认证 | 方案 A + NTLM 认证配置 |
| DNS 可解析但连接失败 | 应用层防火墙 | 方案 A 或方案 B |
| 部分 CDN 可访问 | 白名单规则 | 仅需配置 NO_PROXY 白名单 |

---

## 2. 方案 A — HTTP/HTTPS 代理（推荐）

### 2.1 获取代理信息

向 IT 部门获取以下信息：

```
代理地址:     proxy.example.com 或 proxy.example.com:8080
协议类型:     HTTP / HTTPS / SOCKS5
认证方式:     无认证 / NTLM / Basic / 自动认证
用户名/密码:  （如需要）
PAC 脚本 URL: （如存在，如 http://proxy.example.com/proxy.pac）
```

常见企业代理端口：

| 端口 | 协议 | 说明 |
|------|------|------|
| 8080 | HTTP | 最常见的 HTTP 代理 |
| 3128 | HTTP | Squid 默认端口 |
| 80 | HTTP | 透明代理 |
| 443 | HTTP CONNECT | SSL 代理 |

### 2.2 PowerShell 会话级设置（临时）

```powershell
# 设置代理（无认证）
$env:HTTP_PROXY  = 'http://proxy.example.com:8080'
$env:HTTPS_PROXY = 'http://proxy.example.com:8080'

# 设置代理（有认证）
$env:HTTP_PROXY  = 'http://user:password@proxy.example.com:8080'
$env:HTTPS_PROXY = 'http://user:password@proxy.example.com:8080'

# 设置代理（NTLM 认证——特殊处理，见 §6.2）
$env:HTTP_PROXY  = 'http://proxy.example.com:8080'
$env:HTTPS_PROXY = 'http://proxy.example.com:8080'

# 设置 NO_PROXY——不走代理的内部地址
$env:NO_PROXY = 'localhost,127.0.0.1,::1,.local,.internal,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16'

# 验证设置
$env:HTTP_PROXY
$env:HTTPS_PROXY
$env:NO_PROXY
```

> **注意**: 会话级设置仅在当前 PowerShell 窗口有效，关闭后失效。

### 2.3 系统级代理设置（持久）

#### 方法一：Windows 设置 GUI

```
设置 → 网络和 Internet → 代理 → 手动设置代理
  → 代理服务器: proxy.example.com
  → 端口: 8080
  → 对本地地址不使用代理服务器: 勾选
  → 保存
```

#### 方法二：netsh winhttp（仅影响 WinHTTP 调用）

```powershell
# 查看当前代理
netsh winhttp show proxy

# 设置代理
netsh winhttp set proxy proxy.example.com:8080 "<local>;*.local;*.internal;10.*;172.16.*;172.17.*;172.18.*;172.19.*;172.20.*;172.21.*;172.22.*;172.23.*;172.24.*;172.25.*;172.26.*;172.27.*;172.28.*;172.29.*;172.30.*;172.31.*;192.168.*"

# 重置代理
netsh winhttp reset proxy
```

#### 方法三：系统环境变量（持久化，推荐）

通过 PowerShell 写入用户级环境变量：

```powershell
# 写入用户环境变量（持久化）
[Environment]::SetEnvironmentVariable('HTTP_PROXY',  'http://proxy.example.com:8080', 'User')
[Environment]::SetEnvironmentVariable('HTTPS_PROXY', 'http://proxy.example.com:8080', 'User')
[Environment]::SetEnvironmentVariable('NO_PROXY',    'localhost,127.0.0.1,::1,.local,.internal,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16', 'User')

# 写入系统环境变量（需管理员权限）
[Environment]::SetEnvironmentVariable('HTTP_PROXY',  'http://proxy.example.com:8080', 'Machine')
[Environment]::SetEnvironmentVariable('HTTPS_PROXY', 'http://proxy.example.com:8080', 'Machine')
[Environment]::SetEnvironmentVariable('NO_PROXY',    'localhost,127.0.0.1,::1,.local,.internal,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16', 'Machine')
```

> **生效**: 系统环境变量修改后需要**重启 PowerShell/终端**才能生效。

### 2.4 Git 代理配置

```powershell
# 设置全局 HTTP 代理
git config --global http.proxy http://proxy.example.com:8080
git config --global https.proxy http://proxy.example.com:8080

# 验证
git config --global --get http.proxy
git config --global --get https.proxy

# 仅对 GitHub 设置代理（推荐，避免影响内部 Git 仓库）
git config --global http.https://github.com.proxy http://proxy.example.com:8080
git config --global https.https://github.com.proxy http://proxy.example.com:8080

# 设置 NO_PROXY（Git 不使用系统环境变量，需单独配置）
git config --global http.proxyAuthMethod negotiate  # NTLM 认证方式

# 取消代理
git config --global --unset http.proxy
git config --global --unset https.proxy

# 查看所有代理相关配置
git config --global --list | Select-String proxy
```

### 2.5 Cargo 代理配置

本项目 `.cargo/config.toml` 位于 `D:\Chimera CLI\.cargo\config.toml`，编辑或创建该文件添加代理配置：

```toml
# D:\Chimera CLI\.cargo\config.toml
[http]
proxy = "http://proxy.example.com:8080"

# 如果使用 NTLM 认证，cargo 不支持原生 NTLM，需使用 cntlm 本地转换（见 §6.2）
# [http]
# proxy = "http://localhost:3128"  # cntlm 本地转发端口

# 设置超时（秒），避免代理响应慢导致超时
[http]
timeout = 120
low-speed-limit = 5
low-speed-time = 60
```

> **注意**: Cargo 的代理配置读取 `HTTP_PROXY`/`HTTPS_PROXY` 环境变量，`.cargo/config.toml` 中的 `[http] proxy` 属于第二优先级。环境变量优先。

#### 使用 cargo 镜像源（替代方案）

如果代理不稳定，可配置国内 crates.io 镜像：

```toml
# D:\Chimera CLI\.cargo\config.toml
[source.crates-io]
replace-with = "rsproxy"

[source.rsproxy]
registry = "https://rsproxy.cn/crates.io-index"

# 或使用中科大镜像
# [source.ustc]
# registry = "https://mirrors.ustc.edu.cn/crates.io-index"

# 或使用字节跳动镜像
# [source.bytedance]
# registry = "https://rsproxy.bytefast.cn/crates.io-index"
```

### 2.6 Docker/Podman 代理配置

#### Docker Desktop

```powershell
# Docker Desktop GUI 设置
# 设置 → Resources → Proxies
#   HTTP Proxy:  http://proxy.example.com:8080
#   HTTPS Proxy: http://proxy.example.com:8080
#   No Proxy:    localhost,127.0.0.1,.local,.internal

# 或通过 JSON 配置文件
# 编辑 %USERPROFILE%\.docker\config.json
# {
#   "proxies": {
#     "default": {
#       "httpProxy":  "http://proxy.example.com:8080",
#       "httpsProxy": "http://proxy.example.com:8080",
#       "noProxy":    "localhost,127.0.0.1,.local,.internal"
#     }
#   }
# }
```

#### Podman Machine（Windows Podman）

```powershell
# 进入 Podman 虚拟机
podman machine ssh

# 在虚拟机内设置代理
sudo tee /etc/environment << 'EOF'
HTTP_PROXY=http://proxy.example.com:8080
HTTPS_PROXY=http://proxy.example.com:8080
NO_PROXY=localhost,127.0.0.1,::1,.local,.internal,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16
EOF

# 退出虚拟机
exit

# 重启 Podman 虚拟机
podman machine stop
podman machine start

# 验证代理在容器内生效
podman run --rm alpine:latest wget -O- https://github.com
```

#### Docker 容器构建代理

在项目的 `Dockerfile` 中（如果构建时也需要代理）：

```dockerfile
# 构建时使用代理（--build-arg 传递）
ARG HTTP_PROXY
ARG HTTPS_PROXY
ARG NO_PROXY

# 设置代理（仅在构建阶段有效）
ENV HTTP_PROXY=$HTTP_PROXY
ENV HTTPS_PROXY=$HTTPS_PROXY
ENV NO_PROXY=$NO_PROXY

# 构建完成后清理代理信息（避免进入最终镜像）
# RUN unset HTTP_PROXY HTTPS_PROXY NO_PROXY
```

构建命令：

```powershell
docker build `
  --build-arg HTTP_PROXY=http://proxy.example.com:8080 `
  --build-arg HTTPS_PROXY=http://proxy.example.com:8080 `
  --build-arg NO_PROXY=localhost,127.0.0.1 `
  -t chimera-cli:local .
```

### 2.7 WSL2 代理配置

如果使用 WSL2 进行开发：

```bash
# 在 WSL2 内设置代理
# 获取 Windows 主机 IP
HOST_IP=$(cat /etc/resolv.conf | grep nameserver | awk '{print $2}')

# 临时设置
export HTTP_PROXY="http://$HOST_IP:8080"
export HTTPS_PROXY="http://$HOST_IP:8080"
export NO_PROXY="localhost,127.0.0.1,::1,.local,.internal"

# 持久化设置（追加到 ~/.bashrc 或 ~/.zshrc）
cat >> ~/.bashrc << 'EOF'
HOST_IP=$(cat /etc/resolv.conf | grep nameserver | awk '{print $2}')
export HTTP_PROXY="http://$HOST_IP:8080"
export HTTPS_PROXY="http://$HOST_IP:8080"
export NO_PROXY="localhost,127.0.0.1,::1,.local,.internal,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16"
EOF

# 生效
source ~/.bashrc
```

### 2.8 cargo audit 代理配置

`cargo audit` 依赖网络下载安全 advisory 数据库，需要代理：

```powershell
# 方式一：依赖环境变量（推荐）
$env:HTTP_PROXY  = 'http://proxy.example.com:8080'
$env:HTTPS_PROXY = 'http://proxy.example.com:8080'
cargo audit --deny warnings `
  --ignore RUSTSEC-2026-0190 `
  --ignore RUSTSEC-2026-0002 `
  --ignore RUSTSEC-2024-0436

# 方式二：cargo-audit 自 v0.18+ 支持 --proxy 参数
# cargo audit --proxy http://proxy.example.com:8080 --deny warnings
```

---

## 3. 方案 B — SSH 隧道（备选）

### 3.1 适用场景

- 没有 HTTP 代理可用
- 有 SSH 访问权限的跳板机（堡垒机）
- 跳板机可以访问外网 443 端口

### 3.2 建立 SSH 隧道

```powershell
# 基本 SSH 隧道：将本地 1443 端口转发到跳板机，通过跳板机访问外网
# 格式: ssh -L <本地端口>:<目标主机>:<目标端口> <跳板机>
ssh -L 1443:github.com:443 user@bastion.example.com

# 更实用的动态 SOCKS5 隧道（推荐）
# 本地 SOCKS5 代理监听 1080 端口
ssh -D 1080 user@bastion.example.com

# 保持连接 + 后台运行
ssh -D 1080 -N -f -o ServerAliveInterval=60 user@bastion.example.com
```

### 3.3 使用 SSH 隧道

SOCKS5 隧道建立后（监听 `localhost:1080`），配置工具使用：

```powershell
# PowerShell 设置
$env:HTTP_PROXY  = 'socks5://localhost:1080'
$env:HTTPS_PROXY = 'socks5://localhost:1080'

# Git 设置
git config --global http.proxy socks5://localhost:1080
git config --global https.proxy socks5://localhost:1080

# 测试
curl -x socks5://localhost:1080 https://github.com
```

### 3.4 特定端口转发（精细化）

如果仅需代理特定服务，可建立独立隧道：

```powershell
# 参数说明:
#   -L 1443:github.com:443    → 本地 1443 → GitHub 443
#   -L 2443:crates.io:443     → 本地 2443 → crates.io 443
#   -L 3443:gcr.io:443        → 本地 3443 → gcr.io 443
#   -L 4443:registry.npmjs.org:443 → 本地 4443 → npm registry 443

ssh -L 1443:github.com:443 `
    -L 2443:crates.io:443 `
    -L 3443:gcr.io:443 `
    user@bastion.example.com
```

然后通过修改 hosts 文件 + 本地端口转发使用：

```
# 不推荐，仅作概念说明
# 实际使用建议用 SOCKS5 动态隧道，更灵活
```

### 3.5 SSH 隧道持久化（Windows 服务）

```powershell
# 使用 PowerShell 后台作业
$job = Start-Job -ScriptBlock {
    ssh -D 1080 -N -o ServerAliveInterval=60 user@bastion.example.com
}

# 或使用 Windows 任务计划程序
# 创建一个启动时运行的任务，执行 ssh -D 1080 -N -f user@bastion.example.com
```

---

## 4. 方案 C — 离线操作（无奈之选）

> 当方案 A 和方案 B 均不可行时的最后手段。

### 4.1 离线 crate 依赖缓存

在有网络的机器上：

```powershell
# 1. 在目标机器上生成完整依赖列表
# 在 D:\Chimera CLI 项目根目录下
cargo tree --prefix depth | Out-File -Encoding utf8 deps.txt

# 2. 或者直接生成 Cargo.lock 的依赖清单
# 复制 Cargo.lock 到有网络的机器
```

在有网络的机器上获取所有 crate：

```powershell
# 方法一：使用 cargo vendor
# 在项目目录执行
cargo vendor --versioned-dirs

# 这会在 vendor/ 目录下存放所有依赖的源码
# 将 vendor/ 目录打包传输到离线机器

# 在离线机器上配置使用 vendor 目录
# 编辑 .cargo/config.toml
# [source.vendored-sources]
# directory = "D:\\Chimera CLI\\vendor"
#
# [source.crates-io]
# replace-with = "vendored-sources"
```

```powershell
# 方法二：使用 cargo download
# 安装 cargo-download
cargo install cargo-download

# 下载单个 crate
cargo download serde -s > serde.tar.gz
```

### 4.2 离线 cargo 缓存传输

```powershell
# 在网络机器上构建并缓存
cargo build --workspace

# 缓存位于 %USERPROFILE%\.cargo\registry\cache\
# 或 $env:CARGO_HOME\registry\cache\

# 将整个缓存目录打包传输
# 到离线机器后解压到对应位置

# 注意：本项目 CARGO_HOME = D:\Chimera CLI\.toolchain\cargo
# 缓存应放置到 D:\Chimera CLI\.toolchain\cargo\registry\cache\
```

### 4.3 Docker 镜像离线传输

```powershell
# 步骤一：在有网络的机器上拉取镜像
docker pull gcr.io/distroless/cc-debian12:latest
docker pull rust:1.85-slim

# 步骤二：导出为 tar 文件
docker save gcr.io/distroless/cc-debian12:latest -o distroless-cc-debian12.tar
docker save rust:1.85-slim -o rust-1.85-slim.tar

# 步骤三：通过 USB 或内网传输到离线机器

# 步骤四：在离线机器上导入
docker load -i distroless-cc-debian12.tar
docker load -i rust-1.85-slim.tar

# 验证
docker images | Select-String distroless
```

### 4.4 Podman airgap 安装

```powershell
# 下载 Podman 离线安装包
# 从 https://github.com/containers/podman/releases 下载
# 或从微软官方 winget 下载（如果 winget 可访问）

# 如果没有网络，从 USB 复制 podman.msi 安装包
# 注意：Podman 5.x 需要 22H2 以上 Windows 版本
```

### 4.5 Git 仓库离线克隆

```powershell
# 在有网络的环境
git clone --bare https://github.com/user/repo.git repo.git
# 将 repo.git 目录打包传输

# 在离线环境
git clone repo.git new-repo
cd new-repo
git remote set-url origin https://github.com/user/repo.git
```

---

## 5. 验证步骤

### 5.1 HTTP 代理验证

```powershell
# 基础验证：带代理的 HTTP 请求
curl -x http://proxy.example.com:8080 -I https://github.com

# 如果 curl 不可用，使用 .NET WebClient
$wc = New-Object System.Net.WebClient
$wc.Proxy = New-Object System.Net.WebProxy("http://proxy.example.com:8080")
$wc.DownloadString("https://github.com") | Select-Object -First 5

# 验证代理响应头
Invoke-WebRequest -Uri https://github.com `
  -Proxy http://proxy.example.com:8080 `
  -UseBasicParsing `
  -TimeoutSec 30
```

### 5.2 Git 代理验证

```powershell
# 测试 Git 通信
git ls-remote https://github.com/rust-lang/rust.git HEAD

# 查看 Git 使用代理的详细日志
GIT_CURL_VERBOSE=1 git ls-remote https://github.com/rust-lang/rust.git HEAD

# 测试本项目仓库
git ls-remote https://github.com/user/repo.git HEAD
```

### 5.3 Cargo 代理验证

```powershell
# 验证 cargo 可通过代理访问 crates.io
cargo search serde --limit 1

# 验证 cargo 下载依赖
cargo build --workspace 2>&1 | Select-String -First 10

# 查看 cargo 的 HTTP 调试日志
$env:CARGO_HTTP_DEBUG = 'true'
cargo build --workspace 2>&1 | Select-String proxy
```

### 5.4 Docker/Podman 代理验证

```powershell
# 验证 Docker 拉取
docker pull hello-world:latest

# 验证 Podman 拉取
podman pull hello-world:latest

# 验证容器内代理
docker run --rm alpine:latest sh -c "wget -q -O- https://github.com && echo OK"
```

### 5.5 一键验证脚本

```powershell
# 保存为 test-proxy.ps1
function Test-Proxy {
    param([string]$ProxyUrl)

    $tests = @(
        @{Name="GitHub";         URL="https://github.com"},
        @{Name="crates.io";      URL="https://crates.io"},
        @{Name="gcr.io";         URL="https://gcr.io"},
        @{Name="Docker Hub";     URL="https://docker.io"},
        @{Name="rsproxy.cn";     URL="https://rsproxy.cn"}
    )

    Write-Host "=== 代理验证: $ProxyUrl ===" -ForegroundColor Cyan
    foreach ($t in $tests) {
        try {
            $resp = Invoke-WebRequest -Uri $t.URL `
                -Proxy $ProxyUrl `
                -UseBasicParsing `
                -TimeoutSec 15 `
                -Method HEAD
            Write-Host "  ✓ $($t.Name) - 可达 ($($resp.StatusCode))" -ForegroundColor Green
        } catch {
            Write-Host "  ✗ $($t.Name) - 不可达 ($($_.Exception.Message))" -ForegroundColor Red
        }
    }
}

# 使用示例
Test-Proxy -ProxyUrl "http://proxy.example.com:8080"
```

---

## 6. 故障排查

### 6.1 407 代理认证失败

**现象**: `407 Proxy Authentication Required`

**原因**: 代理服务器要求认证，但未提供或认证方式不匹配。

**解决方案**:

```powershell
# 方式一：在 URL 中嵌入凭证（仅 Basic 认证）
$env:HTTPS_PROXY = 'http://domain\user:password@proxy.example.com:8080'

# 注意：密码中含有特殊字符需要 URL 编码
# @ → %40  # : → %3A  # / → %2F  # \ → %5C
# 例：密码为 "P@ssw0rd:2024" → "P%40ssw0rd%3A2024"
```

### 6.2 NTLM/Kerberos 认证（企业常见）

Cargo、curl 等工具不原生支持 NTLM 认证。**解决方案：使用 cntlm 本地转发代理**。

```powershell
# 步骤一：下载 cntlm
# https://nchc.dl.sourceforge.net/project/cntlm/cntlm/cntlm%200.92.3/cntlm-0.92.3-win32.zip

# 步骤二：配置 cntlm
# 编辑 cntlm.ini 文件
# Username    domain\user
# Domain      domain
# Password    password
# Proxy       proxy.example.com:8080
# Listen      3128
# AuthType    NTLMv2
# PassNTLMv2  <NTLM hash>

# 更好的做法：使用 -H 参数生成 NTLM hash（避免明文密码）
# cntlm -H -u domain\user -d domain
# 输入密码后，将输出的 NTLM hash 写入 cntlm.ini

# 步骤三：启动 cntlm
cntlm -c cntlm.ini

# 步骤四：配置本地工具使用 cntlm（监听 127.0.0.1:3128）
$env:HTTP_PROXY  = 'http://127.0.0.1:3128'
$env:HTTPS_PROXY = 'http://127.0.0.1:3128'
```

**替代方案：使用 `px` (proxy authentication helper)**

```powershell
# px 是 Rust 实现的代理认证工具，支持 NTLM
# cargo install px
# 在代理设置前运行 px 即可自动处理认证
```

### 6.3 SSL 证书拦截问题

企业代理常使用自签名证书进行 SSL 中间人拦截，导致 SSL 证书验证失败。

**现象**: 在使用 curl/cargo 等工具时，提示 `SSL certificate problem: unable to get local issuer certificate`

**解决方案**:

```powershell
# 方案一：获取企业 CA 证书并添加到信任存储
# 1. 从 IT 部门获取企业根证书（.crt 或 .pem 格式）
# 2. 安装到 Windows 信任存储
# 以管理员身份运行：
Import-Certificate -FilePath "C:\path\to\enterprise-ca.crt" `
  -CertStoreLocation Cert:\LocalMachine\Root

# 方案二：配置 Git 使用特定的 CA 证书
git config --global http.sslCAInfo "C:\path\to\enterprise-ca.crt"

# 方案三：配置 cargo 使用特定的 CA 证书
# 在 .cargo/config.toml 中添加
# [http]
# ssl-version = "tlsv1.2"
# 设置 CA 证书文件路径（环境变量）
$env:SSL_CERT_FILE = "C:\path\to\enterprise-ca.crt"
$env:CURL_CA_BUNDLE = "C:\path\to\enterprise-ca.crt"

# 方案四：临时禁用 SSL 验证（仅用于测试，不推荐用于生产）
$env:CARGO_HTTP_CHECK_REVOKE = 'false'
# Git 临时禁用 SSL 验证
$env:GIT_SSL_NO_VERIFY = '1'
```

### 6.4 NO_PROXY 配置不当

**现象**: 内部 Git 仓库、内部服务通过代理访问导致超时。

**检查**:

```powershell
# 查看当前 NO_PROXY 设置
$env:NO_PROXY

# 常见需要加入 NO_PROXY 的地址
# - 企业内部 Git 服务器: git.internal.example.com
# - 内部 crates 镜像: crates.internal.example.com
# - 内部 Docker 镜像仓库: registry.internal.example.com
# - 内部 CI/CD 服务器: ci.internal.example.com
# - 本地和内部网络: localhost,127.0.0.1,::1,.local,.internal,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16
```

### 6.5 代理响应慢/超时

**现象**: 下载速度慢或频繁超时。

**解决方案**:

```powershell
# 增加 cargo 超时设置
# 编辑 .cargo/config.toml
# [http]
# timeout = 300
# low-speed-limit = 1
# low-speed-time = 120

# 使用国内镜像源（见 §2.5）
# 使用多线程下载（如 cargo-binstall）
cargo binstall cargo
```

### 6.6 代理断开后恢复

**现象**: 代理间歇性断开，导致部分操作失败。

**解决方案**:

```powershell
# 设置自动重试
# Git 重试
git config --global http.lowSpeedLimit 0
git config --global http.lowSpeedTime 300  # 5 分钟

# 使用 retry 包装命令
function Invoke-WithRetry {
    param([ScriptBlock]$Command, [int]$Retries = 3)
    for ($i = 0; $i -lt $Retries; $i++) {
        try {
            & $Command
            return
        } catch {
            Write-Warning "第 $($i+1) 次尝试失败: $_"
            Start-Sleep -Seconds 5
        }
    }
    throw "重试 $Retries 次后仍然失败"
}

# 使用示例
Invoke-WithRetry -Command { cargo build --workspace }
```

---

## 7. CI/CD 特殊注意事项

### 7.1 GitHub Actions Runner 代理配置

如果自托管 GitHub Actions Runner 也处于企业网络环境，需要进行以下配置：

#### Windows 自托管 Runner

```powershell
# 为 Actions Runner 设置代理环境变量
# 编辑 runner 启动脚本前添加：

$env:HTTP_PROXY  = 'http://proxy.example.com:8080'
$env:HTTPS_PROXY = 'http://proxy.example.com:8080'
$env:NO_PROXY    = 'localhost,127.0.0.1,::1,.local,.internal,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16'

# 在 config.cmd 中也可配置代理
.\config.cmd --url https://github.com/your-org --token your-token `
  --proxyurl http://proxy.example.com:8080 `
  --proxysslcafile C:\path\to\enterprise-ca.crt
```

#### 在 workflow YAML 中设置代理

```yaml
# .github/workflows/release.yml 中
# 如果使用 GitHub 托管 runner，无法直接设置代理
# 但可以通过环境变量传递

jobs:
  build:
    runs-on: windows-latest
    env:
      HTTP_PROXY:  ${{ secrets.HTTP_PROXY }}
      HTTPS_PROXY: ${{ secrets.HTTPS_PROXY }}
      NO_PROXY:    localhost,127.0.0.1,::1,.local,.internal

    steps:
      - uses: actions/checkout@v4
        env:
          HTTP_PROXY:  ${{ secrets.HTTP_PROXY }}
          HTTPS_PROXY: ${{ secrets.HTTPS_PROXY }}

      - name: Build
        run: cargo build --workspace
        env:
          # 国内镜像源（如果代理不可用）
          CARGO_REGISTRIES_CRATES_IO_PROTOCOL: sparse
```

> **注意**: GitHub 托管的 runner 默认可以访问外网。以上配置仅在**自托管 runner** 位于代理后时需要。

### 7.2 敏感信息处理

**代理凭证不要硬编码到 workflow 文件中**：

```yaml
# ✓ 正确：使用 GitHub Secrets
# 在 GitHub 仓库 Setting → Secrets → Actions 中设置
# 命名: HTTP_PROXY_URL, HTTP_PROXY_USER, HTTP_PROXY_PASS
# 然后在 workflow 中使用
- name: Configure proxy
  run: |
    echo "HTTP_PROXY=http://${{ secrets.HTTP_PROXY_USER }}:${{ secrets.HTTP_PROXY_PASS }}@proxy.example.com:8080" >> $env:GITHUB_ENV
```

### 7.3 Docker 构建在 CI 中的代理

```yaml
# release.yml 中 Docker 构建
- name: Build Docker image
  run: |
    docker build \
      --build-arg HTTP_PROXY=${{ env.HTTP_PROXY }} \
      --build-arg HTTPS_PROXY=${{ env.HTTPS_PROXY }} \
      --build-arg NO_PROXY=${{ env.NO_PROXY }} \
      -t chimera-cli:${{ github.ref_name }} .
```

### 7.4 CI 中 cargo audit 的代理

```yaml
# audit.yml 中
- name: Cargo audit
  run: |
    cargo audit --deny warnings `
      --ignore RUSTSEC-2026-0190 `
      --ignore RUSTSEC-2026-0002 `
      --ignore RUSTSEC-2024-0436
  env:
    HTTP_PROXY:  ${{ secrets.HTTP_PROXY }}
    HTTPS_PROXY: ${{ secrets.HTTPS_PROXY }}
```

### 7.5 内网镜像/缓存策略（推荐用于 CI）

对于自托管 runner，建议搭建内网缓存层，减少对公网代理的依赖：

```yaml
# 使用 actions/cache 缓存 cargo 依赖
- uses: actions/cache@v4
  with:
    path: |
      ~/.cargo/registry
      ~/.cargo/git
      target
    key: ${{ runner.os }}-cargo-${{ hashFiles('**/Cargo.lock') }}
```

**内网镜像推荐架构**：

```
外部网络 ← [代理/GitHub] ← 自托管 Runner
                                    ↓
                              内网缓存服务器
                             ├── crates.io 镜像 (如 Nexus)
                             ├── Docker 镜像代理 (如 Harbor)
                             └── Git 缓存 (如 GitLab Mirror)
```

---

## 附录

### A. 快速参考卡片

| 组件 | 配置方式 | 重启要求 |
|------|----------|---------|
| PowerShell 会话 | `$env:HTTP_PROXY = '...'` | 无（即时生效） |
| 系统环境变量 | `[Environment]::SetEnvironmentVariable` | 新终端生效 |
| Git | `git config --global http.proxy` | 即时生效 |
| Cargo | 环境变量 或 `.cargo/config.toml` | 即时生效 |
| Docker Desktop | 设置 GUI 或 `~/.docker/config.json` | 重启 Docker 生效 |
| Podman | `podman machine ssh` 设置 | 重启 Podman 生效 |
| WSL2 | `~/.bashrc` 或 `~/.zshrc` | 新 shell 生效 |
| GitHub Actions | secrets + workflow env | 每次 job 独立 |

### B. 本项目环境变量参考

```powershell
# 为 Chimera CLI 项目设置完整的代理环境
# 保存为 proxy-env.ps1, 在开发前执行

# 代理基础配置
$proxyHost = 'proxy.example.com'
$proxyPort = '8080'
$proxyUrl  = "http://${proxyHost}:${proxyPort}"

# 设置代理
$env:HTTP_PROXY  = $proxyUrl
$env:HTTPS_PROXY = $proxyUrl

# 设置 NO_PROXY
$env:NO_PROXY = 'localhost,127.0.0.1,::1,.local,.internal,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16'

# 工具链路径（本项目专属）
$env:CARGO_HOME  = 'D:\Chimera CLI\.toolchain\cargo'
$env:RUSTUP_HOME = 'D:\Chimera CLI\.toolchain\rustup'
$env:TMP         = 'D:\Chimera CLI\tmp'
$env:TEMP        = 'D:\Chimera CLI\tmp'

# 设置 SSL 证书（如果企业代理有 SSL 拦截）
# $env:SSL_CERT_FILE = 'C:\path\to\enterprise-ca.crt'
# $env:CURL_CA_BUNDLE = 'C:\path\to\enterprise-ca.crt'

# 验证
Write-Host "代理已配置: $env:HTTPS_PROXY" -ForegroundColor Green
Write-Host "NO_PROXY: $env:NO_PROXY" -ForegroundColor Gray
```

### C. 常见镜像源地址

| 服务 | 官方地址 | 国内镜像 |
|------|---------|---------|
| crates.io | `https://crates.io` | `https://rsproxy.cn` / `https://mirrors.ustc.edu.cn/crates.io-index` |
| crates.io 索引 | `https://github.com/rust-lang/crates.io-index` | `https://rsproxy.cn/crates.io-index` |
| GitHub | `https://github.com` | `https://github.com`（部分镜像可用） |
| gcr.io | `https://gcr.io` | `https://gcr.mirrors.ustc.edu.cn` |
| Docker Hub | `https://docker.io` | `https://docker.mirrors.ustc.edu.cn` / `https://dockerproxy.com` |
| Rust 工具链 | `https://static.rust-lang.org` | `https://rsproxy.cn/rustup` |
| Python PyPI | `https://pypi.org` | `https://pypi.tuna.tsinghua.edu.cn/simple` |

---

> **文档版本**: v1.0  
> **最后更新**: 2026-07-13  
> **适用范围**: Chimera CLI (NEXUS-OMEGA) 项目 Windows 开发环境  
> **相关文件**: `.claude/CLAUDE.md` §1 环境设置, `.trae/rules/nuxus规则.md` §7 开发工作流