// Compile with:
//   gcc message_mutation.c log.c -fpic -shared -o libmessage_mutation.so \
//     -I../../target -L../../target/debug -lempath -L ../../target -lempath

#include <stdio.h>
#include <string.h>
#include <time.h>

#include "empath.h"
#include "log.h"

static void _set_timestamp_header(Context *ctx, const char *header) {
  char timestamp[64] = {0};
  time_t now = time(NULL);
  strftime(timestamp, sizeof(timestamp) - 1, "%FT%TZ", gmtime(&now));
  em_message_headers_set(ctx, header, timestamp);
}

int validate_data(Context *ctx) {
  const Line line = create(ctx, "C message mutation module");
  log(&line);

  em_message_headers_set_if_absent(ctx, "X-Processed-By", "c-message-mutator");
  _set_timestamp_header(ctx, "X-Mutation-Timestamp");
  em_message_headers_append(ctx, "X-Mutation-Flag", "c-mutated");
  em_message_body_append_text(ctx, "\n\n-- mutated by C module --", "utf-8");

  return 0;
}

EM_DECLARE_MODULE(Validation, .validation_listener = {
                                  "message_mutation",
                                  NULL,
                                  {
                                      .validate_data = validate_data,
                                  }});
