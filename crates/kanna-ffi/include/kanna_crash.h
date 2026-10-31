#ifndef KANNA_CRASH_H
#define KANNA_CRASH_H

#include <stdbool.h>

char *kanna_crash_write(const char *request);
bool kanna_crash_written(void);
void kanna_crash_install(const char *state_dir);
void kanna_crash_free(char *string);

#endif
