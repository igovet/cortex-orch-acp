# Local dictation dependencies

- sherpa-onnx-node and the platform-native package are pinned to 2.0.1 by package-lock.json.
  Upstream: https://github.com/k2-fsa/sherpa-onnx/tree/v2.0.1
  Licence: Apache-2.0; see LICENSE-APACHE-2.0. The native package includes ONNX Runtime,
  copyright Microsoft Corporation, under MIT; see LICENSE-ONNXRUNTIME.
- The first-use model is the small Russian Zipformer export dated 2024-09-18.
  The archive and four used files are bound by SHA-256 in model.json.
  Original model card: https://huggingface.co/alphacep/vosk-model-small-ru
  Export instructions: https://k2-fsa.github.io/sherpa/onnx/pretrained_models/offline-transducer/zipformer-transducer-models.html
  The original model card declares Apache-2.0. Model weights are downloaded on demand,
  not redistributed in the Debian package.

The worker is Cortex-owned code. Native recognition packages are copied without source
modification. Bundled Node uses the existing engine bundle and its licence.
This inventory is not a complete audit of native transitive components or public-release approval.
