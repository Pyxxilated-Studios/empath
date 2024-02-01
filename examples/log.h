#include "../target/empath.h"

typedef struct Line {
  const char *id;
  const char *message;
} Line;

Line create(Context *validate_context, const char *msg);

void log(const Line *line);