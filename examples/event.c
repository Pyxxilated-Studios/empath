// Compile with
//   gcc event.c -fpic -shared -o libevent.so -l empath_common -L \
//     ../target/debug
//

#include <stdio.h>

#include "../target/empath/common.h"
#include "../target/empath/smtp/proto.h"

int emit(Ev event, Context *vctx) {
  if (event == ConnectionOpened) {
    printf("Opened connection!\n");
  } else if (event == ConnectionClosed) {
    printf("Closed connection!\n");
  } else {
    printf("Unknown event! %d\n", event);
  }

  return event;
}

int init(StringVector arguments) {
  printf("INIT CALLED\n");

  for (int i = 0; i < arguments.len; i++) {
    printf("Arg: %*s\n", (int)arguments.data[i].len, arguments.data[i].data);
  }

  return 0;
}

EM_DECLARE_MODULE(Event, .event_listener = {"event", init, emit});
