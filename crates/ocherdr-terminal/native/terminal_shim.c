#include <ghostty/vt.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
  GhosttyTerminal terminal;
  GhosttyFormatter formatter;
  uint16_t cols;
  uint16_t rows;
} OcHerdrTerminal;

int ocherdr_terminal_new(uint16_t cols, uint16_t rows, size_t scrollback,
                         OcHerdrTerminal **out) {
  if (out == NULL || cols == 0 || rows == 0) return GHOSTTY_INVALID_VALUE;
  OcHerdrTerminal *state = calloc(1, sizeof(OcHerdrTerminal));
  if (state == NULL) return GHOSTTY_OUT_OF_MEMORY;

  GhosttyTerminalOptions terminal_options = {
      .cols = cols,
      .rows = rows,
      .max_scrollback = scrollback,
  };
  int result = ghostty_terminal_new(NULL, &state->terminal, terminal_options);
  if (result != GHOSTTY_SUCCESS) {
    free(state);
    return result;
  }

  GhosttyFormatterTerminalOptions formatter_options = {0};
  formatter_options.size = sizeof(formatter_options);
  formatter_options.emit = GHOSTTY_FORMATTER_FORMAT_PLAIN;
  formatter_options.unwrap = false;
  formatter_options.trim = false;
  formatter_options.extra.size = sizeof(formatter_options.extra);
  formatter_options.extra.screen.size = sizeof(formatter_options.extra.screen);
  result = ghostty_formatter_terminal_new(NULL, &state->formatter,
                                          state->terminal, formatter_options);
  if (result != GHOSTTY_SUCCESS) {
    ghostty_terminal_free(state->terminal);
    free(state);
    return result;
  }
  state->cols = cols;
  state->rows = rows;
  *out = state;
  return GHOSTTY_SUCCESS;
}

void ocherdr_terminal_free(OcHerdrTerminal *state) {
  if (state == NULL) return;
  ghostty_formatter_free(state->formatter);
  ghostty_terminal_free(state->terminal);
  free(state);
}

void ocherdr_terminal_reset(OcHerdrTerminal *state) {
  if (state != NULL) ghostty_terminal_reset(state->terminal);
}

void ocherdr_terminal_write(OcHerdrTerminal *state, const uint8_t *bytes,
                            size_t length) {
  if (state != NULL && bytes != NULL)
    ghostty_terminal_vt_write(state->terminal, bytes, length);
}

int ocherdr_terminal_resize(OcHerdrTerminal *state, uint16_t cols,
                            uint16_t rows, uint32_t cell_width_px,
                            uint32_t cell_height_px) {
  if (state == NULL) return GHOSTTY_INVALID_VALUE;
  int result = ghostty_terminal_resize(state->terminal, cols, rows,
                                       cell_width_px, cell_height_px);
  if (result == GHOSTTY_SUCCESS) {
    state->cols = cols;
    state->rows = rows;
  }
  return result;
}

size_t ocherdr_terminal_text(OcHerdrTerminal *state, uint8_t *buffer,
                             size_t capacity) {
  if (state == NULL) return 0;
  size_t written = 0;
  int result = ghostty_formatter_format_buf(state->formatter, buffer, capacity,
                                             &written);
  if (result == GHOSTTY_SUCCESS || result == GHOSTTY_OUT_OF_SPACE) return written;
  return 0;
}

uint16_t ocherdr_terminal_cols(const OcHerdrTerminal *state) {
  return state == NULL ? 0 : state->cols;
}

uint16_t ocherdr_terminal_rows(const OcHerdrTerminal *state) {
  return state == NULL ? 0 : state->rows;
}
