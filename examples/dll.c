// Compile with
//   gcc dll.c -shared -o libdll.so -l empath -L ../../target/debug
//

#include <stdio.h>

#include "../target/empath/smtp/proto.h"

void test(ValidationContext *vctx) {
  printf("Hello world!: %s\n", validation_context_get_id(vctx));
}

int init(ValidationContext *vctx) {
  test(vctx);
  Buffer buff = validation_context_get_recipients(vctx);

  for (int i = 0; i < buff.len; i++) {
    printf("Recipient: %s\n", buff.data[i]);
  }

  return buff.len;
}