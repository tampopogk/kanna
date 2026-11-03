#ifndef KANNA_CALLTREE_H
#define KANNA_CALLTREE_H

#include <stddef.h>

char *kanna_calltree_parse(const char *source, size_t len, const char *doc, size_t doc_len);
void kanna_calltree_free(char *json);

#endif
