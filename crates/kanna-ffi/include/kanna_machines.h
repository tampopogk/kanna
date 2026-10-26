#ifndef KANNA_MACHINES_H
#define KANNA_MACHINES_H

char *kanna_machines_client(const char *key_path);
char *kanna_machines_link(const char *request);
char *kanna_machines_path(const char *key);
void kanna_machines_free(char *string);

#endif
