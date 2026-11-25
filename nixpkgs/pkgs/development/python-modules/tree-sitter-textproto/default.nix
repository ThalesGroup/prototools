{
  lib,
  buildPythonPackage,
  fetchFromGitHub,
  setuptools,

  # passthru
  nix-update-script,
}:

buildPythonPackage (finalAttrs: {
  pname = "tree-sitter-textproto";
  version = "0.4.0";
  pyproject = true;
  __structuredAttrs = true;

  src = fetchFromGitHub {
    owner = "douzebis";
    repo = "prototools";
    tag = "v${finalAttrs.version}";
    hash = "sha256-zE8jDoPISjJg56cRjs0LJDDBQv9dWYAryg29RpZLsZU=";
  };

  sourceRoot = "${finalAttrs.src.name}/tree-sitter-textproto";

  build-system = [ setuptools ];

  # There are no tests
  doCheck = false;

  pythonImportsCheck = [ "tree_sitter_textproto" ];

  # Only v-prefixed release tags
  passthru.updateScript = nix-update-script {
    extraArgs = [
      "--version-regex"
      "^v(\\d+\\.\\d+\\.\\d+)$"
    ];
  };

  meta = {
    description = "Textproto grammar for tree-sitter";
    longDescription = ''
      The protobuf text format grammar prototools uses, modified from
      PorterAtGoogle's tree-sitter-textproto to also parse a bare field
      number as a field name.
    '';
    homepage = "https://github.com/douzebis/prototools";
    changelog = "https://github.com/douzebis/prototools/blob/${finalAttrs.src.rev}/CHANGELOG.md";
    license = with lib.licenses; [
      isc
      mit
    ];
    maintainers = with lib.maintainers; [ douzebis ];
  };
})
