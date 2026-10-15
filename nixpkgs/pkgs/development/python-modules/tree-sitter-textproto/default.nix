{
  lib,
  buildPythonPackage,
  fetchFromGitHub,
  setuptools,
}:

buildPythonPackage (finalAttrs: {
  pname = "tree-sitter-textproto";
  version = "0.3.4";
  pyproject = true;

  src = fetchFromGitHub {
    owner = "douzebis";
    repo = "prototools";
    tag = "v${finalAttrs.version}";
    hash = lib.fakeHash;
  };

  sourceRoot = "${finalAttrs.src.name}/reproto/tree-sitter-textproto";

  build-system = [ setuptools ];

  # There are no tests
  doCheck = false;

  pythonImportsCheck = [ "tree_sitter_textproto" ];

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
