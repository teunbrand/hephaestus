/* The Rust boundary, as Swift sees it. Mirrors `src/lib.rs`. */

#ifndef HEPHAESTUS_QUICKLOOK_BRIDGE_H
#define HEPHAESTUS_QUICKLOOK_BRIDGE_H

#include <stddef.h>
#include <stdint.h>

/* A rendered PDF owned by Rust. `data` is NULL when rendering failed, which is
   the only error channel — a preview extension has nothing useful to do with a
   message. `capacity` is carried because reconstructing the Rust allocation
   with the wrong one is undefined behaviour. */
typedef struct {
  uint8_t *data;
  size_t len;
  size_t capacity;
  double width_pt;
  double height_pt;
} HepPdf;

/* Render the plot document in `bytes` to a PDF. The caller owns the result and
   must pass it to `hephaestus_pdf_free`. */
HepPdf hephaestus_pdf_from_document(const uint8_t *bytes, size_t len);

/* Reclaim a result. A no-op when `data` is NULL, so callers can free
   unconditionally. */
void hephaestus_pdf_free(HepPdf pdf);

#endif
