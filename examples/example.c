// Compile with
//   gcc example.c -fpic -shared -o libexample.so -l empath_common -L \
//     ../target/debug
//

#include <stdio.h>

#include "../target/empath/common.h"
#include "../target/empath/smtp/proto.h"

int something = 1;

void test(Context *validate_context) {
  String id = em_context_get_id(validate_context);
  printf("Hello world!: %s\n", id.data);
  free_string(id);
}

int validate_connect(Context *validate_context) {
  em_context_set_data_response(validate_context, "4.2.1 Failure!");

  return 0;
}

int validate_data(Context *validate_context) {
  test(validate_context);
  StringVector buff = em_context_get_recipients(validate_context);

  em_context_set(validate_context, "test", "random");

  for (int i = 0; i < buff.len; i++) {
    printf("Recipient: %s\n", buff.data[i].data);
  }

  printf("Something: %d\n", something);

  int i = em_context_set_sender(validate_context, "test@gmail.com");
  if (i != 0) {
    printf("There was an issue setting the sender\n");
  }

  String sender = em_context_get_sender(validate_context);
  printf("Sender: %s\n", sender.data);

  String data = em_context_get_data(validate_context);
  printf("Data:\n%s\n", data.data);

  if (em_context_set_data_response(validate_context, "Test Response") != 0) {
    printf("Unable to set data response\n");
  }

  if (em_context_exists(validate_context, "test")) {
    String value = em_context_get(validate_context, "test");
    printf("Existing value: %*s\n", (int)value.len, value.data);
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

EM_DECLARE_MODULE(Validation, .validation_listener = {
                                  "dll",
                                  init,
                                  {
                                      .validate_connect = validate_connect,
                                      .validate_data = validate_data,
                                  }});
