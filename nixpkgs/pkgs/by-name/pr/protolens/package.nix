{
  lib,
  stdenv,
  rustPlatform,
  fetchFromGitHub,
  installShellFiles,
  makeWrapper,
  versionCheckHook,
  neovim,
  buf,

  # passthru
  nix-update-script,

  # The editor key opens the descriptor in Neovim, which protolens finds
  # on PATH, and tells the user when it is missing. withNeovim puts one on
  # the wrapper's PATH instead.
  withNeovim ? false,
  # buf's language server gives the editor go-to-definition, references and
  # hover; without it the file opens, highlighted, without navigation.
  withBuf ? false,
}:

let
  # What the editor key needs on the wrapper's PATH: nothing by default.
  editorPath = lib.optional withNeovim neovim ++ lib.optional withBuf buf;
in
rustPlatform.buildRustPackage (finalAttrs: {
  pname = "protolens";
  version = "0.3.2";
  __structuredAttrs = true;

  src = fetchFromGitHub {
    owner = "douzebis";
    repo = "prototools";
    tag = "v${finalAttrs.version}";
    hash = "sha256-wNeO5GUz0XkUpdiIGp4xWXf7Ol2pmfnqspCbeTAKmKs=";
  };

  cargoHash = "sha256-4F/ZRjvaFPTJALcRZ0j8bMwIRAaTTghf4fCbx2+6z3M=";

  # The crate is prototext-lens on crates.io; its program, protolens.
  cargoBuildFlags = [
    "-p"
    "prototext-lens"
  ];
  cargoTestFlags = [
    "-p"
    "prototext-lens"
  ];

  nativeBuildInputs = [
    installShellFiles
    makeWrapper
  ];

  postInstall =
    lib.optionalString (stdenv.buildPlatform.canExecute stdenv.hostPlatform) ''
      installShellCompletion --cmd protolens \
        --bash <(PROTOLENS_COMPLETE=bash $out/bin/protolens) \
        --zsh <(PROTOLENS_COMPLETE=zsh $out/bin/protolens) \
        --fish <(PROTOLENS_COMPLETE=fish $out/bin/protolens)
      PROTOLENS_GEN_MAN=$out/share/man/man1 $out/bin/protolens
    ''
    + ''
      install -Dm444 protolens/nvim/init.lua $out/share/protolens/nvim/init.lua
      wrapProgram $out/bin/protolens \
        --set PROTOLENS_NVIM_CONFIG $out/share/protolens/nvim/init.lua ${
          # No --prefix when empty: an empty PATH entry means the
          # current directory.
          lib.optionalString (editorPath != [ ]) "--prefix PATH : ${lib.makeBinPath editorPath}"
        }
    '';

  nativeInstallCheckInputs = [ versionCheckHook ];
  doInstallCheck = true;

  # Only v-prefixed release tags: the repository also has workshop tags
  # (grehack2026-*), which are not versions.
  passthru.updateScript = nix-update-script {
    extraArgs = [
      "--version-regex"
      "^v(\\d+\\.\\d+\\.\\d+)$"
    ];
  };

  meta = {
    description = "Terminal viewer to decode, navigate and extract binary protobuf messages";
    longDescription = ''
      protolens shows a binary protobuf message as a tree, decoded with or
      without its schema, and lets you navigate it, inspect each field's
      bytes, and extract them. Its editor key opens the field's declaration
      in Neovim; with buf installed, Neovim can then navigate the schema.
    '';
    homepage = "https://github.com/douzebis/prototools";
    changelog = "https://github.com/douzebis/prototools/blob/${finalAttrs.src.rev}/CHANGELOG.md";
    license = lib.licenses.mit;
    maintainers = with lib.maintainers; [ douzebis ];
    mainProgram = "protolens";
    platforms = lib.platforms.unix;
  };
})
