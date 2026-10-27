class Luuna < Formula
  desc "Local-first development workbench command-line suite"
  homepage "https://github.com/luuna-ai/luuna"
  version "0.1.0"
  license "MIT"

  # These immutable Git revisions are refreshed together with
  # components/manifest.json. The component resources must exactly match its
  # pinned source commits before the build can proceed.
  url "https://github.com/luuna-ai/luuna.git", revision: "aeaaf204a16d1c0073e87afab02422119ea56150"

  resource "missions" do
    url "https://github.com/luuna-ai/luuna-missions.git", revision: "456bbcf07934d557d14fa3469f1c59ac5e4ec1c8"
  end

  resource "atlas" do
    url "https://github.com/luuna-ai/luuna-atlas.git", revision: "76957848c5fa319aef3558956927e069e73ca3e2"
  end

  resource "beacon" do
    url "https://github.com/luuna-ai/luuna-beacon.git", revision: "1c34973a7fba3d0ee1d2aa697f8e1a03cf57f927"
  end

  resource "bun" do
    on_macos do
      on_arm do
        url "https://github.com/oven-sh/bun/releases/download/bun-v1.4.3/bun-darwin-aarch64.zip"
        sha256 "80afd4c069b46a6fa8f3ec520bebc750a7843e4a8d5b9583d29ad436c00a3403"
      end
      on_intel do
        url "https://github.com/oven-sh/bun/releases/download/bun-v1.4.3/bun-darwin-x64.zip"
        sha256 "62660e6438eb872278877acca77e1152d5d031324df368d4a81b428211489d2d"
      end
    end
    on_linux do
      on_arm do
        url "https://github.com/oven-sh/bun/releases/download/bun-v1.4.3/bun-linux-aarch64.zip"
        sha256 "efa9813da5ed72423bf847f916e8d2c47c0d776add972354026a75e10da9aa21"
      end
      on_intel do
        url "https://github.com/oven-sh/bun/releases/download/bun-v1.4.3/bun-linux-x64.zip"
        sha256 "3e730528d89775f03f87f05c06787c4df4099d0a37f3f252e7b829cb23816e18"
      end
    end
  end

  depends_on "git" => :build
  depends_on "go" => :build
  depends_on "node" => :build
  depends_on "rust" => :build

  def install
    resources.each do |source|
      next if source.name == "bun"

      source.stage do
        (buildpath/"components").mkpath
        destination = buildpath/"components"/source.name
        rm_rf destination
        cp_r Pathname.pwd, destination
      end
    end

    bun_path = nil
    resource("bun").stage do
      bun_path = Dir.glob("**/bun").find { |path| File.file?(path) }
      odie "Bun archive did not contain the bun executable" unless bun_path
      cp bun_path, buildpath/"luuna-bun"
      chmod 0755, buildpath/"luuna-bun"
    end

    # Fail closed on a stale toolchain rather than letting Go download its own
    # toolchain or Bun resolve a moving runtime version.
    ENV["GOTOOLCHAIN"] = "local"
    ENV["GOSUMDB"] = "sum.golang.org"
    ENV["GOCACHE"] = (buildpath/".go-cache").to_s
    ENV["GOMODCACHE"] = (buildpath/".go-mod-cache").to_s
    ENV["BUN_INSTALL_CACHE_DIR"] = (buildpath/".bun-install-cache").to_s
    ENV["BUN_RUNTIME_TRANSPILER_CACHE_PATH"] = (buildpath/".bun-transpiler-cache").to_s
    ENV["LUUNA_BUN"] = (buildpath/"luuna-bun").to_s
    rust_version = Utils.popen_read("rustc", "--version").match(/\brustc (\d+)\.(\d+)\.(\d+)/)
    odie "Rust 1.90.0 or newer is required" unless rust_version && (rust_version[1].to_i > 1 || (rust_version[1].to_i == 1 && rust_version[2].to_i >= 90))
    system "node", buildpath/"scripts/stage-components.mjs", "stage-homebrew",
      "--root", buildpath.to_s, "--out", libexec.to_s, "--bun", ENV.fetch("LUUNA_BUN")
    system "cargo", "build", "--locked", "--release", "-p", "luuna-runtime", "-p", "luuna-tui"
    (libexec/"bin").install "target/release/luuna-runtime", "target/release/luuna-tui"

    # These native Go commands are also installed as direct commands. Luuna's
    # runtime adapters use the verified Bun portable layout staged above.
    { "atlas" => ["luuna-atlas", "lat", "la", "src/cli.ts"],
      "missions" => ["luuna-missions", "lms", "lm", "src/index.ts"],
      "beacon" => ["luuna-beacon", "lbc", "lb", "src/index.ts"] }.each do |name, values|
      entrypoint = values.pop
      aliases = values
      aliases.each do |command|
        (bin/command).write <<~SH
          #!/bin/sh
          set -eu
          exec "#{opt_prefix}/libexec/bun/bun" "#{opt_prefix}/libexec/components/#{name}/#{entrypoint}" "$@"
        SH
        chmod 0755, bin/command
      end
    end

    # Link the TUI under its supported short and long command names.
    bin.install_symlink libexec/"bin/luuna-tui" => "lu"
    bin.install_symlink libexec/"bin/luuna-tui" => "luuna"

    # The runtime needs its component root and exact bundled Bun at every
    # launch. Keep the wrapper Homebrew-relocatable through opt_prefix.
    (bin/"luuna-runtime").write <<~SH
      #!/bin/sh
      set -eu
      export LUUNA_COMPONENTS_ROOT="#{opt_prefix}/libexec"
      export LUUNA_BUN="#{opt_prefix}/libexec/bun/bun"
      exec "#{opt_prefix}/libexec/bin/luuna-runtime" "$@"
    SH
    chmod 0755, bin/"luuna-runtime"
    bin.install "packaging/homebrew-luuna/scripts/luuna-doctor"
    chmod 0755, bin/"luuna-doctor"
  end

  test do
    assert_match(/usage|commands|help/i, shell_output("#{bin}/lu help 2>&1", 0))
    assert_predicate bin/"luuna-runtime", :executable?
    %w[luuna-missions lms lm luuna-atlas lat la luuna-beacon lbc lb].each do |command|
      assert_match(/usage|commands|help/i, shell_output("#{bin}/#{command} --help 2>&1", 0))
    end
    assert_predicate libexec/"components/dist-manifest.json", :file?
    assert_match "Luuna CLI installation check", shell_output("#{bin}/luuna-doctor 2>&1", 0)
  end
end
