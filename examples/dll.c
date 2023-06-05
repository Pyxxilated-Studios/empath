// Compile with
//   gcc dll.c -shared -o libdll.so -l empath -L ../../target/debug
//

#include <stdio.h>

#include "../target/empath/common.h"
#include "../target/empath/smtp/proto.h"

void test(ValidationContext *vctx) {
  FFIString id = validation_context_get_id(vctx);
  printf("Hello world!: %s\n", id.data);
  free_string(id);
}

int init(ValidationContext *vctx) {
  test(vctx);
  FFIStringVector buff = validation_context_get_recipients(vctx);

  for (int i = 0; i < buff.len; i++) {
    printf("Recipient: %s\n", buff.data[i].data);
  }

  free_string_vector(buff);

  return buff.len;
}