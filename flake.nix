{
  description = "tmux-legion - a tmux sidebar tracking every AI agent: blocked, working, done";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "aarch64-darwin"
        "x86_64-darwin"
        "aarch64-linux"
        "x86_64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
      # Single source of truth for the version: Cargo.toml.
      version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
    in
    {
      packages = forAllSystems (pkgs: rec {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "tmux-legion";
          inherit version;
          # Only what cargo consumes: docs/CI/skill commits then don't
          # invalidate the build.
          src = nixpkgs.lib.fileset.toSource {
            root = ./.;
            fileset = nixpkgs.lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./src
            ];
          };
          cargoLock.lockFile = ./Cargo.lock;
          # Integration tests need a live tmux server
          doCheck = false;
          # The full repo, for consumers of non-Rust assets (SKILL.md,
          # claude/hooks.json, pi/tmux-legion.ts) — `src` above is filtered
          # to Rust inputs only and must not be referenced for those.
          passthru.repo = self;
          meta = {
            description = "A tmux sidebar tracking every AI agent: blocked, working, done";
            homepage = "https://github.com/hawkish/tmux-legion";
            license = pkgs.lib.licenses.mit;
            mainProgram = "tmux-legion";
          };
        };

        tmuxPlugin = pkgs.tmuxPlugins.mkTmuxPlugin {
          pluginName = "tmux-legion";
          inherit version;
          src = self;
          rtpFilePath = "tmux-legion.tmux";
          postInstall = ''
            mkdir -p $target/bin
            cp ${default}/bin/tmux-legion $target/bin/tmux-legion
          '';
          meta = default.meta;
        };
      });

      overlays.default = final: prev: {
        tmux-legion = self.packages.${final.stdenv.hostPlatform.system}.default;
        tmuxPlugins = prev.tmuxPlugins // {
          tmux-legion = self.packages.${final.stdenv.hostPlatform.system}.tmuxPlugin;
        };
      };

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            rust-analyzer
            clippy
            rustfmt
            tmux
          ];
        };
      });
    };
}
