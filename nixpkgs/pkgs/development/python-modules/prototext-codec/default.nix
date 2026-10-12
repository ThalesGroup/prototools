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
  version = "0.3.3";
  pyproject = true;

  src = fetchFromGitHub {
    owner = "douzebis";
    repo = "prototools";
    tag = "v${finalAttrs.version}";
    hash = "sha256-SDgv/vrm7mJs1fjKPsn5zrp9s7rDj3W5qPdNt5OU/PE=";
  };

  cargoDeps = rustPlatform.fetchCargoVendor {
    inherit (finalAttrs) pname version src;
    hash = "sha256-DXbYSztud6x+xWDM2dgwkHr88p1MtBR+122Jxe8axls=";
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
    changelog = "https://github.com/douzebis/prototools/blob/${finalAttrs.src.tag}/CHANGELOG.md";
    license = lib.licenses.mit;
    maintainers = with lib.maintainers; [ douzebis ];
  };
})
