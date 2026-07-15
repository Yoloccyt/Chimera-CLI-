class Chimera < Formula
  desc "Chimera CLI — next-gen AI programming agent CLI (NEXUS-OMEGA)"
  homepage "https://github.com/Yoloccyt/Chimera-CLI-"
  version "1.7.0-omega"
  license "MIT"

  if Hardware::CPU.arm?
    url "https://github.com/Yoloccyt/Chimera-CLI-/releases/download/v#{version}/chimera-macos-aarch64"
    sha256 "bbbb76197ea0b1a9298998e969b46025cd17a8c4fa53de37ba730bed02f4e5b6"

    livecheck do
      url :stable
      strategy :github_latest
    end
  else
    url "https://github.com/Yoloccyt/Chimera-CLI-/releases/download/v#{version}/chimera-macos-x86_64"
    sha256 "58c4940c7b984340a7350b9cdd8e48e24717bc6c29462b61ad090cfffc05da84"
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
