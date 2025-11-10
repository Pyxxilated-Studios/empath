// Compile with
//   gcc event.c -fpic -shared -o libevent.so -l empath -L ../../target
//

#include <stdio.h>

#include "empath.h"

int emit(Ev event, Context *validate_context) {
  if (event == ConnectionOpened) {
    printf("Opened connection!\n");
  } else if (event == ConnectionClosed) {
    printf("Closed connection!\n");
  } else if (event == DeliveryAttempt) {
    printf("Delivery attempt started\n");

    if (em_context_has_delivery(validate_context)) {
      String domain = em_delivery_get_domain(validate_context);
      printf("  Domain: %*s\n", (int)domain.len, domain.data);
      em_free_string(domain);

      String server = em_delivery_get_server(validate_context);
      if (server.len > 0) {
        printf("  Server: %*s\n", (int)server.len, server.data);
      }
      em_free_string(server);
    }
  } else if (event == DeliverySuccess) {
    printf("Delivery succeeded!\n");

    if (em_context_has_delivery(validate_context)) {
      String domain = em_delivery_get_domain(validate_context);
      printf("  Domain: %*s\n", (int)domain.len, domain.data);
      em_free_string(domain);
    }
  } else if (event == DeliveryFailure) {
    printf("Delivery failed!\n");

    if (em_context_has_delivery(validate_context)) {
      String error = em_delivery_get_error(validate_context);
      if (error.len > 0) {
        printf("  Error: %*s\n", (int)error.len, error.data);
      }
      em_free_string(error);

      uint32_t attempts = em_delivery_get_attempts(validate_context);
      printf("  Attempts: %u\n", attempts);
    }
  } else {
    printf("Unknown event! %d\n", event);
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

  for (int i = 0; i < arguments.len; i++) {
    printf("Arg: %*s\n", (int)arguments.data[i].len, arguments.data[i].data);
  }

  return 0;
}

EM_DECLARE_MODULE(Event, .event_listener = {"event", init, emit});
