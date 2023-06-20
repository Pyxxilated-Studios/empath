// Compile with
//   gcc dll.c -shared -o libdll.so -l empath -L ../target/debug
//

#include <stdio.h>

#include "empath/common.h"
#include "empath/smtp/proto.h"

int something = 1;

void test(Context *vctx) {
  String id = context_get_id(vctx);
  printf("Hello world!: %s\n", id.data);
  free_string(id);
}

int validate_data(Context *vctx) {
  test(vctx);
  StringVector buff = context_get_recipients(vctx);

  for (int i = 0; i < buff.len; i++) {
    printf("Recipient: %s\n", buff.data[i].data);
  }

  printf("Something: %d\n", something);

  int i = context_set_sender(vctx, "test@gmail.com");
  if (i != 0) {
    printf("There was an issue setting the sender\n");
  }

  String sender = context_get_sender(vctx);
  printf("Sender: %s\n", sender.data);

  free_string(sender);
  free_string_vector(buff);

  return 0;
}

int init() {
  printf("INIT CALLED\n");
  something = 2;
  return 0;
}