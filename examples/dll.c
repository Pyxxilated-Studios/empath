// Compile with
//   gcc dll.c -shared -o libdll.so -l empath -L ../../target/debug
//

#include <stdio.h>

#include "../../target/empath.h"

void test(ValidationContext *vctx) {
  printf("Hello world!: %s\n", validation_context_get_id(vctx));
}

int init(ValidationContext *vctx) {
  test(vctx);
  return validation_context_get_recipients(vctx).len;
}