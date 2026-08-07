class Chimera < Formula
  desc "Chimera CLI — next-gen AI programming agent CLI (NEXUS-OMEGA)"
  homepage "https://github.com/Yoloccyt/Chimera-CLI-"
  version "2.21.0-omega"
  license "MIT"

  # 2026-08-07 v2.21.0-omega 同步:sha256 取自 GitHub Release checksums.txt(发布后同步)。

  if Hardware::CPU.arm?
    url "https://github.com/Yoloccyt/Chimera-CLI-/releases/download/v#{version}/chimera-macos-aarch64"
    sha256 "d320fa63a16bb2d4de9da8d1bb954d9e5f5beb1b24b1ecd00d8a9d11dbf43f1c"

    livecheck do
      url :stable
      strategy :github_latest
    end
  else
    url "https://github.com/Yoloccyt/Chimera-CLI-/releases/download/v#{version}/chimera-macos-x86_64"
    sha256 "7426dfd1ae470f97eca2991a4c280bafd5f3902897eff964d1398d1a3bd905dd"
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
