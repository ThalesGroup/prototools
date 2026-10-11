{
  lib,
  buildPythonPackage,
  fetchFromGitHub,
  rustPlatform,
  protobuf,
  pytestCheckHook,
}:

buildPythonPackage (finalAttrs: {
  pname = "prototext-codec";
  version = "0.3.2";
  pyproject = true;

  src = fetchFromGitHub {
    owner = "douzebis";
    repo = "prototools";
    tag = "v${finalAttrs.version}";
    hash = "sha256-wNeO5GUz0XkUpdiIGp4xWXf7Ol2pmfnqspCbeTAKmKs=";
  };

  cargoDeps = rustPlatform.fetchCargoVendor {
    inherit (finalAttrs) pname version src;
    hash = "sha256-4F/ZRjvaFPTJALcRZ0j8bMwIRAaTTghf4fCbx2+6z3M=";
  };

  buildAndTestSubdir = "prototext-pyo3";

  nativeBuildInputs = [ rustPlatform.cargoSetupHook ];

  build-system = [ rustPlatform.maturinBuildHook ];

  nativeCheckInputs = [
    protobuf
    pytestCheckHook
  ];

  enabledTestPaths = [ "prototext-pyo3/tests" ];

  pythonImportsCheck = [ "prototext_codec_lib" ];

  meta = {
    description = "Python bindings to prototext's lossless protobuf codec";
    homepage = "https://github.com/douzebis/prototools";
    changelog = "https://github.com/douzebis/prototools/blob/${finalAttrs.src.rev}/CHANGELOG.md";
    license = lib.licenses.mit;
    maintainers = with lib.maintainers; [ douzebis ];
  };
})
