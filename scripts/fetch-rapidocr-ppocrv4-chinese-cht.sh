#!/usr/bin/env bash
# Fetch the pinned RapidOCR ONNX weights for the image OCR chain.
# PP-OCRv4 mobile, recognition chinese_cht (chinese_cht_PP-OCRv3_rec_mobile)
# and chinese_cht_dict. Not PP-OCRv6. No simplified-Chinese conversion.
# Unit tests and CI do not run this script. Do not commit the downloaded files.
set -euo pipefail

DEST="${1:-${HOME}/.cache/ocrs/rapidocr}"
mkdir -p "$DEST"

fetch() {
  local url="$1" name="$2" sha="$3"
  local dest="$DEST/$name"
  if [[ -f "$dest" ]]; then
    echo "${sha}  ${dest}" | sha256sum -c -
    return
  fi
  curl -fsSL --retry 3 --max-time 180 -o "$dest" "$url"
  echo "${sha}  ${dest}" | sha256sum -c -
}

# RapidOCR default_models.yaml v3.9.2, onnxruntime / PP-OCRv4 (not the v6 default).
fetch \
  "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/onnx/PP-OCRv4/det/ch_PP-OCRv4_det_mobile.onnx" \
  "ch_PP-OCRv4_det_mobile.onnx" \
  "d2a7720d45a54257208b1e13e36a8479894cb74155a5efe29462512d42f49da9"
fetch \
  "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/onnx/PP-OCRv4/rec/chinese_cht_PP-OCRv3_rec_mobile.onnx" \
  "chinese_cht_PP-OCRv3_rec_mobile.onnx" \
  "779656d044ce388045e02ea9244724616194e63928606436cdfc6dc3c9528cc6"
# Dict published beside that rec model. SHA256 of the file at that URL.
fetch \
  "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/paddle/PP-OCRv4/rec/chinese_cht_PP-OCRv3_rec_mobile/chinese_cht_dict.txt" \
  "chinese_cht_dict.txt" \
  "832551fee1f2fbc97508772d81ebdc8dba12c00de97a35c71c9ddf43ddac1a83"

echo "Pinned PP-OCRv4 mobile chinese_cht into $DEST"
echo "Point [ocr].model_dir at the parent of this rapidocr directory (default ~/.cache/ocrs)."
