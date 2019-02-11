#include <emscripten.h>
#include <tree_sitter/api.h>
#include <stdio.h>

void **TRANSFER_BUFFER = NULL;
size_t TRANSFER_BUFFER_SIZE = 0;

typedef struct {
  size_t length;
  char bytes[10 * 1024];
} TSParserInputBuffer;

static void ensure_transfer_buffer(size_t size) {
  if (TRANSFER_BUFFER_SIZE < size) {
    TRANSFER_BUFFER = realloc(TRANSFER_BUFFER, size * sizeof(void *));
    TRANSFER_BUFFER_SIZE = size;
  }
}

void **ts_parser_new_wasm() {
  ensure_transfer_buffer(2);
  TSParser *parser = ts_parser_new();
  TSParserInputBuffer *input_buffer = malloc(sizeof(TSParserInputBuffer));
  TRANSFER_BUFFER[0] = parser;
  TRANSFER_BUFFER[1] = input_buffer;
  return TRANSFER_BUFFER;
}

void ts_parser_delete_wasm(TSParser *parser, char *input_buffer) {
  ts_parser_delete(parser);
  free(input_buffer);
}

typedef void (*InputCallback)(
  uint32_t byte,
  uint32_t row,
  uint32_t column
);

typedef struct {
  TSParserInputBuffer *buffer;
  InputCallback callback;
} TSParserWasmContext;

static const char *call_js_callback(
  void *payload,
  uint32_t byte,
  TSPoint position,
  uint32_t *bytes_read
) {
  TSParserWasmContext *context = (TSParserWasmContext *)payload;
  context->callback(byte, position.row, position.column);
  *bytes_read = context->buffer->length;
  return context->buffer->bytes;
}

TSTree *ts_parser_parse_wasm(
  TSParser *self,
  TSParserInputBuffer *input_buffer,
  const TSTree *old_tree,
  void *js_callback
) {
  TSParserWasmContext context = {input_buffer, js_callback};
  TSInput input = {&context, call_js_callback, TSInputEncodingUTF16};
  return ts_parser_parse(self, NULL, input);
}
