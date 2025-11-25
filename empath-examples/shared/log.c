
#include <stdio.h>
#include <stdlib.h>
#include <time.h>

#include "log.h"

static bool seeded = false;

const char ID[] = "mid";

Line create(Context *validate_context, const char *msg) {

  Line line;

  if (!em_context_exists(validate_context, ID)) {
    puts("Seeding rand...");

    char id[17] = {0};
    if (!seeded) {
      srandom((unsigned)time(NULL));
      seeded = true;
    }

    const unsigned v = random() & 0xFFFFFFFF;
    sprintf(id, "%016X", v);
    em_context_set(validate_context, ID, id);
  }

  if (em_context_exists(validate_context, "service")) {
    String s = em_context_get(validate_context, "service");
    line.service = (const char *)s.data;
  } else {
    line.service = "";
  }

  String v = em_context_get(validate_context, ID);
  line.id = (const char *)v.data;
  line.message = msg;
  return line;
}

void log(const Line *line) {
  const time_t ti = time(NULL);
  char t[100] = {0};
  strftime(t, sizeof(t), "%FT%TZ", gmtime(&ti));

  printf("[ \"%s\", \"%s\", \"%s\", \"%s\" ]\n", t, line->id, line->service,
         line->message);
  fflush(stdout);
}