class Luuna < Formula
  desc "Local-first development workbench command-line suite"
  homepage "https://github.com/luuna-ai/luuna"
  version "0.1.0"
  license "MIT"

  # One immutable revision of the one repository. Missions, Atlas and Beacon
  # are native Rust libraries inside it (ADR 0066), so no component
  # resources, Bun or Go are needed.
  url "https://github.com/luuna-ai/luuna.git", revision: "aeaaf204a16d1c0073e87afab02422119ea56150"

  depends_on "rust" => :build
  depends_on "git"

  def install
    rust_version = Utils.popen_read("rustc", "--version").match(/\brustc (\d+)\.(\d+)\.(\d+)/)
    odie "Rust 1.90.0 or newer is required" unless rust_version && (rust_version[1].to_i > 1 || (rust_version[1].to_i == 1 && rust_version[2].to_i >= 90))
    system "cargo", "build", "--locked", "--release", "-p", "luuna-runtime", "-p", "luuna-tui", "-p", "luuna-sandbox"
    (libexec/"bin").install "target/release/luuna-runtime", "target/release/luuna-tui", "target/release/luuna-sbx"

    # The terminal client answers to its own names and to the component names
    # (ADR 0053): each is a link to the one executable.
    %w[lu luuna luuna-missions lms lm luuna-atlas lat la luuna-beacon lbc lb].each do |command|
      bin.install_symlink libexec/"bin/luuna-tui" => command
    end
    bin.install_symlink libexec/"bin/luuna-runtime" => "luuna-runtime"
    bin.install "packaging/homebrew-luuna/scripts/luuna-doctor"
    chmod 0755, bin/"luuna-doctor"
  end

  test do
    assert_match(/usage|commands|help/i, shell_output("#{bin}/lu help 2>&1", 0))
    assert_predicate bin/"luuna-runtime", :executable?
    %w[luuna-missions lms lm luuna-atlas lat la luuna-beacon lbc lb].each do |command|
      assert_match(/usage|commands|help/i, shell_output("#{bin}/#{command} --help 2>&1", 0))
    end
    assert_match "Luuna CLI installation check", shell_output("#{bin}/luuna-doctor 2>&1", 0)
  end
end
