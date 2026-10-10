#include <stdio.h>
#include <stdlib.h>
#include <cjson/cJSON.h>
#include "banner.h"

/* Reads JSON on standard input and prints it indented. */
int main(void) {
    char buffer[65536];
    size_t length = fread(buffer, 1, sizeof buffer - 1, stdin);
    buffer[length] = '\0';
    cJSON *json = cJSON_Parse(buffer);
    if (!json) {
        fprintf(stderr, "%s: not valid JSON\n", BANNER);
        return 1;
    }
    char *text = cJSON_Print(json);
    puts(text);
    free(text);
    cJSON_Delete(json);
    return 0;
}
