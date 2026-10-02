#include <stdint.h>

volatile uint32_t counter;
volatile uint32_t result;
uint32_t initialised = 0xC0FFEE; /* .data: copied from flash by the startup code */

static uint32_t fib(uint32_t n)
{
    return n < 2 ? n : fib(n - 1) + fib(n - 2);
}

#define UART0_DR (*(volatile uint32_t *)0x4000C000)
#define SYST_CSR (*(volatile uint32_t *)0xE000E010)
#define SYST_RVR (*(volatile uint32_t *)0xE000E014)

static void put(const char *s)
{
    while (*s) UART0_DR = (uint8_t)*s++;
}

static void put_number(uint32_t n)
{
    char digits[11];
    int i = 10;
    digits[i] = 0;
    do {
        digits[--i] = (char)('0' + n % 10);
        n /= 10;
    } while (n);
    put(&digits[i]);
}

int main(void)
{
    SYST_RVR = 0x00FFFFFF;
    SYST_CSR = 5; /* ENABLE | CLKSOURCE: the core clock */
    for (;;) {
        counter++;
        result = fib(counter % 10);
        if (counter % 20000 == 0) {
            put("tick ");
            put_number(counter);
            put("\n");
        }
    }
}
