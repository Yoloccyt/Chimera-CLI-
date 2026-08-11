class Chimera < Formula
  desc "Chimera CLI — next-gen AI programming agent CLI (NEXUS-OMEGA)"
  homepage "https://github.com/Yoloccyt/Chimera-CLI-"
  version "2.26.0-omega"
  license "MIT"

  # 2026-08-09 v2.25.0-omega 同步:sha256 取自 GitHub Release checksums.txt(发布后同步)。

  if Hardware::CPU.arm?
    url "https://github.com/Yoloccyt/Chimera-CLI-/releases/download/v#{version}/chimera-macos-aarch64"
    sha256 "23113abdb5da8884deed2e4e46167a1720f522f5caabc0644a5aa8616df0a81b"

    livecheck do
      url :stable
      strategy :github_latest
    end
  else
    url "https://github.com/Yoloccyt/Chimera-CLI-/releases/download/v#{version}/chimera-macos-x86_64"
    sha256 "2c1fe666807db3e3844c6bcd8e73ac2ff2d80800748b1097fc71843f99e156c9"
  end

  def install
    # 根据平台确定 binary 文件名,主入口为 chimera
    binary_name = "chimera-macos-#{Hardware::CPU.arch}"
    bin.install binary_name => "chimera"
    # 兼容别名:chimela(旧品牌) / aether(内部编码名)
    bin.install_symlink "chimera" => "chimela"
    bin.install_symlink "chimera" => "aether"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/chimera --version")
    assert_match version.to_s, shell_output("#{bin}/chimela --version")
    assert_match version.to_s, shell_output("#{bin}/aether --version")
  end
end
