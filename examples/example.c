// Compile with
//   gcc example.c -fpic -shared -o libexample.so -l empath -L ../target/debug
//

#include <stdio.h>
#include <string.h>

#include "../target/empath.h"
#include "log.h"

int something = 1;

void test(Context *validate_context) {
  String id = em_context_get_id(validate_context);
  const Line line = create(validate_context, (const char *)id.data);
  log(&line);
  em_free_string(id);
}

int validate_connect(Context *validate_context) {
  const Line line = create(validate_context, "Validating Connection");
  log(&line);
  return 0;
}

int validate_starttls(Context *validate_context) {
  const Line line = create(validate_context, "Validating STARTTLS");
  log(&line);

  if (!em_context_is_tls(validate_context)) {
    return 1;
  }

  char proto[64] = {0};
  snprintf(proto, sizeof(proto), "TLS Protocol: %s",
           (const char *)em_context_tls_protocol(validate_context).data);
  const Line p = create(validate_context, proto);
  log(&p);

  char cipher[128] = {0};
  snprintf(cipher, sizeof(cipher), "TLS Cipher: %s",
           (const char *)em_context_tls_cipher(validate_context).data);
  const Line c = create(validate_context, cipher);
  log(&c);

  return 0;
}

int validate_data(Context *validate_context) {
  test(validate_context);

  em_context_set(validate_context, "test", "random");

  StringVector buff = em_context_get_recipients(validate_context);
  for (int i = 0; i < buff.len; i++) {
    char rcpt[128] = {0};
    snprintf(rcpt, sizeof(rcpt), "Recipient: %s", buff.data[i].data);
    const Line r = create(validate_context, rcpt);
    log(&r);
  }

  em_free_string_vector(buff);

  printf("Something: %d\n", something);

  String sender0 = em_context_get_sender(validate_context);
  char sdr[128] = {0};
  snprintf(sdr, sizeof(sdr), "Sender: %s\n", sender0.data);
  const Line s0 = create(validate_context, sdr);
  log(&s0);

  int i = em_context_set_sender(validate_context, "tester@gmail.com");
  if (i == false) {
    printf("There was an issue setting the sender\n");
  }

  String sender = em_context_get_sender(validate_context);
  snprintf(sdr, sizeof(sdr), "Sender: %s\n", sender.data);
  const Line s1 = create(validate_context, sdr);
  log(&s1);
  em_free_string(sender);

  String data = em_context_get_data(validate_context);
  char dat[128] = {0};
  snprintf(dat, sizeof(dat) - 1, "Data:\n%s\n", data.data);
  const Line d = create(validate_context, dat);
  log(&d);
  em_free_string(data);

  String id = em_context_get(validate_context, "mid");
  char resp[64] = {0};
  snprintf(resp, sizeof(resp), "OK [%s]", id.data);
  em_free_string(id);

  if (!em_context_set_response(validate_context, 250, resp)) {
    printf("Unable to set data response\n");
  }

  if (!strcmp((const char *)em_context_get_sender(validate_context).data,
              "test@gmail.com")) {
    em_context_set_response(validate_context, 421, "4.2.1 Failure!");
    return 1;
  }

  if (em_context_exists(validate_context, "test")) {
    String value = em_context_get(validate_context, "test");
    printf("Existing value: %*s\n", (int)value.len, value.data);
    em_free_string(value);
  }

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
                                      .validate_starttls = validate_starttls,
                                  }});
