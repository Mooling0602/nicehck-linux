{
  description = "Native Linux control for NICEHCK / YUANDAO USB-C DSP earphones";

  # Follows your system's nixpkgs registry pin, so no extra input fetching is
  # needed. Override with `inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";`
  # if you want a specific revision.
  inputs.nixpkgs.url = "nixpkgs";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (pkgs:
        let
          inherit (pkgs) lib;

          # Runtime libraries egui/eframe needs on Linux.
          runtimeLibs = with pkgs; [
            libGL
            libxkbcommon
            wayland
            libx11
            libxcursor
            libxi
            libxrandr
            fontconfig
            freetype
          ];

          nicehck-linux = pkgs.rustPlatform.buildRustPackage {
            pname = "nicehck-linux";
            version = "0.1.0";
            src = lib.cleanSource ./.;

            cargoLock.lockFile = ./Cargo.lock;

            nativeBuildInputs = with pkgs; [
              pkg-config
              makeWrapper
            ];

            buildInputs = runtimeLibs;

            # The GUI dlopen()s GL/wayland at runtime, so point it at the store
            # paths explicitly rather than relying on the ambient environment.
            postInstall = ''
              for prog in nicehck nicehck-gui; do
                wrapProgram "$out/bin/$prog" \
                  --prefix LD_LIBRARY_PATH : "${lib.makeLibraryPath runtimeLibs}"
              done
            '';

            # The suite reads /sys/class/hidraw and /dev, which are absent or
            # inaccessible inside the Nix sandbox. Run `cargo test --workspace`
            # on the host instead; see README.
            doCheck = false;

            meta = with lib; {
              description = "Native Linux control for NICEHCK / YUANDAO USB-C DSP earphones";
              license = licenses.mit;
              platforms = platforms.linux;
              mainProgram = "nicehck-gui";
            };
          };

          # `services.udev.packages` scans each package for
          # {etc,lib}/udev/rules.d/*.rules, so the rules need their own derivation.
          # Passing the binary package there would silently grant nothing.
          udevRules = pkgs.runCommand "nicehck-udev-rules" { } ''
            install -Dm644 ${./udev/70-nicehck.rules} \
              "$out/etc/udev/rules.d/70-nicehck.rules"
          '';
        in
        {
          default = nicehck-linux;
          nicehck-linux = nicehck-linux;
          inherit udevRules;
        });

      apps = forAllSystems (pkgs:
        let
          default = {
            type = "app";
            program = "${self.packages.${pkgs.system}.default}/bin/nicehck-gui";
          };
        in
        {
          inherit default;
          gui = default;
          cli = {
            type = "app";
            program = "${self.packages.${pkgs.system}.default}/bin/nicehck";
          };
        });

      # Two independent switches, because the permission rule and the binaries are
      # separate concerns: someone building from source only wants the udev rule.
      #
      #   programs.nicehck.udevRules = true;   # grant hidraw access
      #   programs.nicehck.install   = true;   # also put the binaries on PATH
      nixosModules.default = { config, lib, pkgs, ... }: {
        options.programs.nicehck = {
          enable = lib.mkEnableOption "NICEHCK / YUANDAO headset support (both switches below)";

          udevRules = lib.mkOption {
            type = lib.types.bool;
            default = config.programs.nicehck.enable;
            defaultText = lib.literalExpression "config.programs.nicehck.enable";
            description = ''
              Install the udev rule that lets the logged-in user reach the
              headset's vendor HID interface, so no sudo and no manual `cp` is
              needed. This alone is enough if you run the tool from a build tree.
            '';
          };

          install = lib.mkOption {
            type = lib.types.bool;
            default = config.programs.nicehck.enable;
            defaultText = lib.literalExpression "config.programs.nicehck.enable";
            description = ''
              Add `nicehck` and `nicehck-gui` to the system profile.
            '';
          };
        };

        config = {
          services.udev.packages =
            lib.mkIf config.programs.nicehck.udevRules
              [ self.packages.${pkgs.stdenv.hostPlatform.system}.udevRules ];

          environment.systemPackages =
            lib.mkIf config.programs.nicehck.install
              [ self.packages.${pkgs.stdenv.hostPlatform.system}.default ];
        };
      };

      # Import into a nix-darwin/home-manager-style attrset as needed.
      overlays.default = final: prev: {
        nicehck-linux = self.packages.${final.system}.default;
      };

      devShells = forAllSystems (pkgs:
        let
          runtimeLibs = with pkgs; [
            libGL
            libxkbcommon
            wayland
            libx11
            libxcursor
            libxi
            libxrandr
            fontconfig
            freetype
          ];
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              cargo
              rustc
              clippy
              rustfmt
              rust-analyzer
              pkg-config
            ] ++ runtimeLibs;

            shellHook = ''
              echo "nicehck-linux dev shell"
              echo "  cargo test --workspace              run the suite"
              echo "  cargo run -p nicehck-gui            launch the GUI"
              echo "  cargo run -p nicehck-cli -- list    list devices"
            '';
          };
        });

      formatter = forAllSystems (pkgs: pkgs.nixpkgs-fmt);
    };
}
