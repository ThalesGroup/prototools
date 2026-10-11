{
  lib,
  stdenv,
  python3Packages,
  fetchFromGitHub,
  installShellFiles,
  protobuf,
  buf,
  prototext,
  versionCheckHook,

  # passthru
  nix-update-script,
}:

python3Packages.buildPythonApplication (finalAttrs: {
  pname = "reproto";
  version = "0.3.1";
  pyproject = true;

  src = fetchFromGitHub {
    owner = "douzebis";
    repo = "prototools";
    tag = "v${finalAttrs.version}";
    hash = lib.fakeHash;
  };

  sourceRoot = "${finalAttrs.src.name}/reproto";

  build-system = [ python3Packages.setuptools ];

  nativeBuildInputs = [ installShellFiles ];

  dependencies = with python3Packages; [
    click
    fdp-scan
    google-re2
    python3Packages.protobuf
    prototext-codec
    prototext-graph
    pyvis
    pyyaml
    rapidfuzz
    rich
    tree-sitter
    tree-sitter-textproto
  ];

  # The tests compile their fixtures with protoc, normalize the .proto
  # files they render with buf, and compare with prototext's decoding.
  nativeCheckInputs = [
    buf
    protobuf
    prototext
  ]
  ++ (with python3Packages; [
    pytest-xdist
    pytestCheckHook
    tree-sitter-language-pack
  ]);

  postInstall = ''
    installShellCompletion --cmd reproto --bash src/reproto/completions.sh
  ''
  + lib.optionalString (stdenv.buildPlatform.canExecute stdenv.hostPlatform) ''
    $out/bin/reproto-gen-man $out/share/man/man1
  '';

  nativeInstallCheckInputs = [ versionCheckHook ];

  pythonImportsCheck = [ "reproto" ];

  # Only v-prefixed release tags: the repository also has workshop tags
  # (grehack2026-*), which are not versions.
  passthru.updateScript = nix-update-script {
    extraArgs = [
      "--version-regex"
      "^v(\\d+\\.\\d+\\.\\d+)$"
    ];
  };

  meta = {
    description = "Reconstruct .proto sources and schema databases from protobuf descriptors";
    longDescription = ''
      reproto turns compiled protobuf descriptor sets (.pb, or the
      descriptors protoscan finds in a binary) back into .proto source
      files, and builds the schema databases prototext and protolens use to
      decode messages without their original sources.
    '';
    homepage = "https://github.com/douzebis/prototools";
    changelog = "https://github.com/douzebis/prototools/blob/${finalAttrs.src.rev}/CHANGELOG.md";
    license = lib.licenses.mit;
    maintainers = with lib.maintainers; [ douzebis ];
    mainProgram = "reproto";
    platforms = lib.platforms.unix;
  };
})
