#!/usr/bin/env bash
# Downloads the ONNX models visionkit uses into models/ (or $1), pinned to
# a Hugging Face revision and checked against sha256. Re-running skips the
# files that are already there and correct.
#
#   ocr/det.onnx, ocr/det.yml   PP-OCRv5 mobile text detection (Apache-2.0)
#   ocr/rec.onnx, ocr/rec.yml   PP-OCRv5 mobile recognition, zh/ja/en (Apache-2.0);
#                               rec.yml carries the character dictionary
#   cover/dinov2-small.onnx     DINOv2 ViT-S/14 image embeddings (Apache-2.0)
set -euo pipefail

DEST=${1:-"$(cd "$(dirname "$0")/.." && pwd)/models"}
HF=https://huggingface.co

# path  repo  revision  file  sha256
MODELS=(
  "ocr/det.onnx PaddlePaddle/PP-OCRv5_mobile_det_onnx e6f4fa85f00e168c862bc462aebca69eef9b3d3d inference.onnx a431985659dc921974177a95adcfbb90fd9e51989a5e04d70d0b75f597b6e61d"
  "ocr/det.yml PaddlePaddle/PP-OCRv5_mobile_det_onnx e6f4fa85f00e168c862bc462aebca69eef9b3d3d inference.yml 98069072e1b6b37d727fd9d9f11725faa46d6ea0de012f2ed26caea011c37699"
  "ocr/rec.onnx PaddlePaddle/PP-OCRv5_mobile_rec_onnx ed152b8b495f84de93cda5709d768548a9127622 inference.onnx da72dc72ca4dc220df0dfde68c1dedc31c58d3e76a25871122e5056227d50092"
  "ocr/rec.yml PaddlePaddle/PP-OCRv5_mobile_rec_onnx ed152b8b495f84de93cda5709d768548a9127622 inference.yml 5dfeb2777f6d0db8177d8128a8acfcf6e6276dc4ac73ea3bf0dc06d6a5e85d8e"
  "cover/dinov2-small.onnx onnx-community/dinov2-small 8b1f705a3a7f6f062f6bdd21986c1583d3ef105d onnx/model.onnx f22797eabf810a75e41de68d378541ebea372122b25c4ce3ef25ff618250c20a"
)
sha() { if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }

for line in "${MODELS[@]}"; do
  read -r path repo rev file want <<<"$line"
  out="$DEST/$path"
  if [[ -f $out && $(sha "$out") == "$want" ]]; then
    echo "ok        $path"
    continue
  fi
  mkdir -p "$(dirname "$out")"
  echo "download  $path"
  curl -fsSL --retry 3 -o "$out.part" "$HF/$repo/resolve/$rev/$file"
  got=$(sha "$out.part")
  if [[ $got != "$want" ]]; then
    echo "checksum mismatch for $path: $got" >&2
    rm -f "$out.part"
    exit 1
  fi
  mv "$out.part" "$out"
done
