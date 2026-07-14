class Chimela < Formula
  desc "Aether CLI — next-gen AI programming agent CLI (NEXUS-OMEGA)"
  homepage "https://github.com/Yoloccyt/Chimera-CLI-"
  version "1.7.0-omega"
  license "MIT"

  if Hardware::CPU.arm?
    url "https://github.com/Yoloccyt/Chimera-CLI-/releases/download/v#{version}/chimela-macos-aarch64"
    sha256 ""

    livecheck do
      url :stable
      strategy :github_latest
    end
  else
    url "https://github.com/Yoloccyt/Chimera-CLI-/releases/download/v#{version}/chimela-macos-x86_64"
    sha256 ""
  end

  def install
    # 根据平台确定 binary 文件名
    binary_name = "chimela-macos-#{Hardware::CPU.arch}"
    bin.install binary_name => "chimela"
    # 兼容别名
    bin.install_symlink "chimela" => "aether"
    bin.install_symlink "chimela" => "chimera"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/chimela --version")
    assert_match version.to_s, shell_output("#{bin}/aether --version")
    assert_match version.to_s, shell_output("#{bin}/chimera --version")
  end
end
