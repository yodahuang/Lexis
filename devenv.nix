{ pkgs, lib, config, inputs, ... }:

{
  packages = [
    pkgs.claude-code
    pkgs.curl
  ];

  languages.rust.enable = true;
  languages.javascript = {
    enable = true;
    bun.enable = true;
  };

  # Development commands
  scripts.dev.exec = "cd desktop && cargo tauri dev";
  scripts.build.exec = "cd desktop && cargo tauri build";

  # Testing
  scripts.test.exec = "cd desktop/src-tauri && cargo test";
  scripts.setup-test-fixtures.exec = ''
    mkdir -p desktop/src-tauri/tests/fixtures
    FIXTURE_PATH="desktop/src-tauri/tests/fixtures/pride_and_prejudice.txt"

    if [ -f "$FIXTURE_PATH" ]; then
      echo "Test fixture already exists at $FIXTURE_PATH"
    else
      echo "Downloading Pride and Prejudice from Project Gutenberg..."
      curl -sL "https://www.gutenberg.org/cache/epub/1342/pg1342.txt" -o "$FIXTURE_PATH"
      echo "Downloaded to $FIXTURE_PATH ($(wc -c < "$FIXTURE_PATH") bytes)"
    fi
  '';
}
