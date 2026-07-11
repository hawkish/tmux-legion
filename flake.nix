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
    in
    {
      packages = forAllSystems (pkgs: rec {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "tmux-legion";
          version = "0.1.0";
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
          meta = {
            description = "A tmux sidebar tracking every AI agent: blocked, working, done";
            homepage = "https://github.com/hawkish/tmux-legion";
            license = pkgs.lib.licenses.mit;
            mainProgram = "tmux-legion";
          };
        };

        tmuxPlugin = pkgs.tmuxPlugins.mkTmuxPlugin {
          pluginName = "tmux-legion";
          version = "0.1.0";
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
