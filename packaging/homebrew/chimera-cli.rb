class ChimeraCli < Formula
  desc "Chimera CLI (NEXUS-OMEGA) terminal interface"
  homepage "https://github.com/Yoloccyt/Chimera-CLI-"
  version "1.5.3-omega"
  license "Apache-2.0"

  if OS.mac? && Hardware::CPU.arm?
    url "https://github.com/Yoloccyt/Chimera-CLI-/releases/download/v1.5.3-omega/chimera-macos-aarch64"
    sha256 "PLACEHOLDER_SHA256_MACOS_AARCH64"
  elsif OS.mac? && Hardware::CPU.intel?
    url "https://github.com/Yoloccyt/Chimera-CLI-/releases/download/v1.5.3-omega/chimera-macos-x86_64"
    sha256 "PLACEHOLDER_SHA256_MACOS_X86_64"
  elsif OS.linux? && Hardware::CPU.intel?
    url "https://github.com/Yoloccyt/Chimera-CLI-/releases/download/v1.5.3-omega/chimera-linux-x86_64"
    sha256 "PLACEHOLDER_SHA256_LINUX_X86_64"
  else
    odie "Unsupported platform"
  end

  def install
    if OS.mac? && Hardware::CPU.arm?
      bin.install "chimera-macos-aarch64" => "chimera"
    elsif OS.mac? && Hardware::CPU.intel?
      bin.install "chimera-macos-x86_64" => "chimera"
    elsif OS.linux? && Hardware::CPU.intel?
      bin.install "chimera-linux-x86_64" => "chimera"
    end
  end

  test do
    assert_match /^chimera \d+\.\d+\.\d+/, shell_output("#{bin}/chimera --version")
  end
end
