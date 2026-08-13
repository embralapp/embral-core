/* pyke's prebuilt onnxruntime is compiled against glibc >= 2.38, where C23
 * renamed the strtol family to __isoc23_*. The release lane links on glibc
 * 2.35 (ubuntu-22.04, the declared floor), which has no such symbols, so
 * provide them as forwarders. The C23 difference is 0b-prefix parsing,
 * which onnxruntime's number parsing never relies on.
 *
 * Compiled with -std=gnu11 so <stdlib.h> cannot redirect these calls back
 * to the __isoc23_* names on newer glibc, which would recurse. */
#include <stdlib.h>

long __isoc23_strtol(const char *nptr, char **endptr, int base) {
    return strtol(nptr, endptr, base);
}

long long __isoc23_strtoll(const char *nptr, char **endptr, int base) {
    return strtoll(nptr, endptr, base);
}

unsigned long __isoc23_strtoul(const char *nptr, char **endptr, int base) {
    return strtoul(nptr, endptr, base);
}

unsigned long long __isoc23_strtoull(const char *nptr, char **endptr, int base) {
    return strtoull(nptr, endptr, base);
}
