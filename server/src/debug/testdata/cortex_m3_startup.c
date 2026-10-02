#include <stdint.h>
extern uint32_t _etext, _sdata, _edata, _sbss, _ebss, _estack;
int main(void);

void Default_Handler(void) { for (;;) {} }

void Reset_Handler(void)
{
    uint32_t *src = &_etext, *dst = &_sdata;
    while (dst < &_edata) *dst++ = *src++;
    for (dst = &_sbss; dst < &_ebss;) *dst++ = 0;
    main();
    for (;;) {}
}

__attribute__((section(".vectors"), used))
void (*const vector_table[])(void) = {
    (void (*)(void))&_estack, Reset_Handler,
    Default_Handler, Default_Handler, Default_Handler, Default_Handler, Default_Handler,
};
