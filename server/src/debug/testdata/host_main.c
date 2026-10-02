#include <stdio.h>

volatile int counter;
int total = 100;

static int add(int a, int b)
{
    return a + b;
}

int main(void)
{
    for (int i = 0; i < 3; i++) {
        counter++;
        total = add(total, counter);
    }
    printf("total=%d\n", total);
    return 0;
}
