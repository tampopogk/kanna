#ifndef KANNA_FLEET_H
#define KANNA_FLEET_H

char *kanna_fleet_client(const char *key_path);
char *kanna_fleet_link(const char *request);
char *kanna_fleet_path(const char *key);
void kanna_fleet_free(char *string);

#endif
