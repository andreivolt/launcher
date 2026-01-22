{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";
    crane.url = "github:ipetkov/crane";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils, crane }:
    let
      # System-specific outputs
      systemOutputs = flake-utils.lib.eachDefaultSystem (system:
        let
          overlays = [ (import rust-overlay) ];
          pkgs = import nixpkgs { inherit system overlays; };
          rustToolchain = pkgs.rust-bin.stable.latest.default;

          craneLib = (crane.mkLib pkgs).overrideToolchain rustToolchain;

          # Common build inputs
          buildInputs = with pkgs; [
            wayland
            libxkbcommon
            vulkan-loader
            fontconfig
            freetype
          ];
          nativeBuildInputs = with pkgs; [ pkg-config makeWrapper ];

          # Build deps only (for caching)
          cargoArtifacts = craneLib.buildDepsOnly {
            src = craneLib.cleanCargoSource ./.;
            inherit buildInputs nativeBuildInputs;
          };

          # Build the actual package
          launcher = craneLib.buildPackage {
            inherit cargoArtifacts buildInputs nativeBuildInputs;
            src = craneLib.cleanCargoSource ./.;
            doCheck = false; # Skip tests for faster builds

            postInstall = ''
              wrapProgram $out/bin/launcher \
                --prefix LD_LIBRARY_PATH : ${pkgs.lib.makeLibraryPath [
                  pkgs.wayland
                  pkgs.libxkbcommon
                  pkgs.vulkan-loader
                  pkgs.fontconfig
                ]}
            '';
          };
        in {
          devShells.default = pkgs.mkShell {
            buildInputs = buildInputs ++ (with pkgs; [
              rustToolchain
              pkg-config
              roboto
              bacon
              cargo-watch
              process-compose
            ]);

            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath [
              pkgs.wayland
              pkgs.libxkbcommon
              pkgs.vulkan-loader
              pkgs.fontconfig
            ];
          };

          packages.default = launcher;
        }
      );
    in
    systemOutputs // {
      nixosModules.default = { config, lib, pkgs, ... }:
        let
          cfg = config.services.launcher;
          launcherPkg = self.packages.${pkgs.system}.default;
        in {
          options.services.launcher = {
            enable = lib.mkEnableOption "launcher service";
          };

          config = lib.mkIf cfg.enable {
            environment.systemPackages = [ launcherPkg ];

            systemd.user.services.launcher = {
              description = "Launcher";
              wantedBy = [ "hyprland-session.target" ];
              partOf = [ "hyprland-session.target" ];
              after = [ "hyprland-session.target" ];
              path = [ pkgs.hyprland pkgs.util-linux ];
              serviceConfig = {
                ExecStart = "${launcherPkg}/bin/launcher";
                Restart = "on-failure";
                RestartSec = 2;
                PassEnvironment = "HYPRLAND_INSTANCE_SIGNATURE XDG_RUNTIME_DIR WAYLAND_DISPLAY";
              };
            };
          };
        };
    };
}
