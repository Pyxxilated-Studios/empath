// Compile with
//   gcc example.c -fpic -shared -o libexample.so -l empath_common -L \
//     ../target/debug
//

#include <stdio.h>

#include "../target/empath/common.h"
#include "../target/empath/smtp/proto.h"

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

  String data = context_get_data(vctx);
  printf("Data:\n%s\n", data.data);

  if (context_set_data_response(vctx, "Test Response") != 0) {
    printf("Unable to set data response\n");
  }

  free_string(sender);
  free_string(data);
  free_string_vector(buff);

  return 0;
}

int init(StringVector arguments) {
  printf("INIT CALLED\n");
  something = 2;

  for (int i = 0; i < arguments.len; i++) {
    printf("Arg: %*s\n", (int)arguments.data[i].len, arguments.data[i].data);
  }

  return 0;
}

EM_DECLARE_MODULE(Validation, .validation_listener = {"dll",
                                                      init,
                                                      {
                                                          validate_data,
                                                      }});
