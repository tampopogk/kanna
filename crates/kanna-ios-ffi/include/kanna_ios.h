#ifndef KANNA_IOS_H
#define KANNA_IOS_H

char *kanna_phone_start(const char *secret);
char *kanna_phone_join(const char *offer);
char *kanna_phone_instance(const char *key);
char *kanna_phone_key(const char *secret);
char *kanna_phone_open(const char *request);
char *kanna_phone_write(unsigned long long stream, const char *line);
char *kanna_phone_read(unsigned long long stream);
char *kanna_phone_path(const char *key);
char *kanna_phone_highlight(const char *file, const char *text);
void kanna_phone_stop(void);
void kanna_phone_free(char *text);

#endif
