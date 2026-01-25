{ pkgs, lib, config, inputs, ... }:

{
  packages = [
    pkgs.claude-code
  ];

  languages.rust.enable = true;
  languages.javascript = {
    enable = true;
    bun.enable = true;
  };

  scripts.dev.exec = "cd desktop && cargo tauri dev";
  scripts.build.exec = "cd desktop && cargo tauri build";
}
