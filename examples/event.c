// Compile with
//   gcc event.c -fpic -shared -o libevent.so -l empath -L ../target/debug
//

#include <stdio.h>

#include "../target/empath.h"

int emit(Ev event, Context *validate_context) {
  if (event == ConnectionOpened) {
    printf("Opened connection!\n");
  } else if (event == ConnectionClosed) {
    printf("Closed connection!\n");
  } else {
    printf("Unknown event! %d\n", event);
  }

  if (em_context_exists(validate_context, "test")) {
    String value = em_context_get(validate_context, "test");
    printf("Existing value: %*s\n", (int)value.len, value.data);
    free_string(value);
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
