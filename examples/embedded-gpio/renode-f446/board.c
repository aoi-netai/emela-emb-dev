#include "stm32f4xx_hal.h"
#include "emela_embedded.h"

/* サンプルで使う論理 GPIO と NUCLEO-F446RE 上の物理ピンを対応付ける。 */
enum {
    EMELA_BUTTON_PIN = 2,
    EMELA_LED_PIN = 13,
};

static uint8_t hal_initialized;

/* newlib の初期化配列を呼ぶために必要な空の起動フック。 */
void _init(void) {}
void _fini(void) {}

static void board_fault(void) {
    __disable_irq();
    for (;;) {
        __WFI();
    }
}

static void board_clock(void) {
    RCC_OscInitTypeDef oscillators = {0};
    RCC_ClkInitTypeDef buses = {0};

    __HAL_RCC_PWR_CLK_ENABLE();
    __HAL_PWR_VOLTAGESCALING_CONFIG(PWR_REGULATOR_VOLTAGE_SCALE3);

    oscillators.OscillatorType = RCC_OSCILLATORTYPE_HSI;
    oscillators.HSIState = RCC_HSI_ON;
    oscillators.HSICalibrationValue = RCC_HSICALIBRATION_DEFAULT;
    oscillators.PLL.PLLState = RCC_PLL_ON;
    oscillators.PLL.PLLSource = RCC_PLLSOURCE_HSI;
    oscillators.PLL.PLLM = 16;
    oscillators.PLL.PLLN = 336;
    oscillators.PLL.PLLP = RCC_PLLP_DIV4;
    oscillators.PLL.PLLQ = 2;
    oscillators.PLL.PLLR = 2;
    if (HAL_RCC_OscConfig(&oscillators) != HAL_OK) {
        board_fault();
    }

    buses.ClockType = RCC_CLOCKTYPE_SYSCLK | RCC_CLOCKTYPE_HCLK
                    | RCC_CLOCKTYPE_PCLK1 | RCC_CLOCKTYPE_PCLK2;
    buses.SYSCLKSource = RCC_SYSCLKSOURCE_PLLCLK;
    buses.AHBCLKDivider = RCC_SYSCLK_DIV1;
    buses.APB1CLKDivider = RCC_HCLK_DIV2;
    buses.APB2CLKDivider = RCC_HCLK_DIV1;
    if (HAL_RCC_ClockConfig(&buses, FLASH_LATENCY_2) != HAL_OK) {
        board_fault();
    }
}

static void board_init(void) {
    if (HAL_Init() != HAL_OK) {
        board_fault();
    }
    board_clock();
    /* このイベントサンプルでは HAL の時間待ちを使わないため SysTick を止める。 */
    HAL_SuspendTick();
    hal_initialized = 1;
}

static void gpio_location(EmelaGpioPin pin, GPIO_TypeDef **port,
                          uint16_t *physical_pin) {
    switch (pin) {
    case EMELA_BUTTON_PIN:
        *port = GPIOC;
        *physical_pin = GPIO_PIN_13;
        __HAL_RCC_GPIOC_CLK_ENABLE();
        return;
    case EMELA_LED_PIN:
        *port = GPIOA;
        *physical_pin = GPIO_PIN_5;
        __HAL_RCC_GPIOA_CLK_ENABLE();
        return;
    default:
        board_fault();
    }
}

static void ensure_initialized(void) {
    if (!hal_initialized) {
        board_init();
    }
}

void emela_gpio_configure(EmelaGpioPin pin, EmelaGpioMode mode,
                          EmelaGpioPull pull) {
    GPIO_TypeDef *port;
    uint16_t physical_pin;
    GPIO_InitTypeDef config = {0};

    ensure_initialized();
    gpio_location(pin, &port, &physical_pin);
    config.Pin = physical_pin;
    config.Speed = GPIO_SPEED_FREQ_LOW;

    switch (mode) {
    case EMELA_GPIO_MODE_INPUT:
        config.Mode = GPIO_MODE_INPUT;
        break;
    case EMELA_GPIO_MODE_OUTPUT_PUSH_PULL:
        config.Mode = GPIO_MODE_OUTPUT_PP;
        break;
    default:
        board_fault();
    }

    switch (pull) {
    case EMELA_GPIO_PULL_NO_PULL:
        config.Pull = GPIO_NOPULL;
        break;
    case EMELA_GPIO_PULL_UP:
        config.Pull = GPIO_PULLUP;
        break;
    case EMELA_GPIO_PULL_DOWN:
        config.Pull = GPIO_PULLDOWN;
        break;
    default:
        board_fault();
    }

    HAL_GPIO_Init(port, &config);
}

EmelaPinState emela_gpio_read(EmelaGpioPin pin) {
    GPIO_TypeDef *port;
    uint16_t physical_pin;
    ensure_initialized();
    gpio_location(pin, &port, &physical_pin);
    return HAL_GPIO_ReadPin(port, physical_pin) == GPIO_PIN_SET
        ? EMELA_PIN_STATE_HIGH : EMELA_PIN_STATE_LOW;
}

void emela_gpio_write(EmelaGpioPin pin, EmelaPinState state) {
    GPIO_TypeDef *port;
    uint16_t physical_pin;
    ensure_initialized();
    gpio_location(pin, &port, &physical_pin);
    if (pin != EMELA_LED_PIN) {
        board_fault();
    }
    HAL_GPIO_WritePin(port, physical_pin,
                      state == EMELA_PIN_STATE_HIGH ? GPIO_PIN_SET : GPIO_PIN_RESET);
}

void emela_gpio_toggle(EmelaGpioPin pin) {
    GPIO_TypeDef *port;
    uint16_t physical_pin;
    ensure_initialized();
    gpio_location(pin, &port, &physical_pin);
    if (pin != EMELA_LED_PIN) {
        board_fault();
    }
    HAL_GPIO_TogglePin(port, physical_pin);
}

void emela_gpio_enable_interrupt(EmelaGpioPin pin, EmelaGpioEdge edge) {
    GPIO_InitTypeDef config = {0};

    ensure_initialized();
    if (pin != EMELA_BUTTON_PIN) {
        board_fault();
    }

    config.Pin = GPIO_PIN_13;
    config.Pull = GPIO_PULLUP;
    config.Speed = GPIO_SPEED_FREQ_LOW;
    switch (edge) {
    case EMELA_GPIO_EDGE_RISING:
        config.Mode = GPIO_MODE_IT_RISING;
        break;
    case EMELA_GPIO_EDGE_FALLING:
        config.Mode = GPIO_MODE_IT_FALLING;
        break;
    case EMELA_GPIO_EDGE_BOTH:
        config.Mode = GPIO_MODE_IT_RISING_FALLING;
        break;
    default:
        board_fault();
    }

    HAL_GPIO_Init(GPIOC, &config);
    HAL_NVIC_SetPriority(EXTI15_10_IRQn, 0, 0);
    HAL_NVIC_EnableIRQ(EXTI15_10_IRQn);
}

void emela_gpio_disable_interrupt(EmelaGpioPin pin) {
    if (pin != EMELA_BUTTON_PIN) {
        board_fault();
    }
    HAL_NVIC_DisableIRQ(EXTI15_10_IRQn);
}

void EXTI15_10_IRQHandler(void) {
    HAL_GPIO_EXTI_IRQHandler(GPIO_PIN_13);
}

void HAL_GPIO_EXTI_Callback(uint16_t physical_pin) {
    if (physical_pin == GPIO_PIN_13) {
        EmelaGpioEvent event = {
            .pin = EMELA_BUTTON_PIN,
            .level = HAL_GPIO_ReadPin(GPIOC, GPIO_PIN_13) == GPIO_PIN_SET
                ? EMELA_PIN_STATE_HIGH : EMELA_PIN_STATE_LOW,
        };
        emela_export_button_changed(event);
    }
}

void SysTick_Handler(void) {
    HAL_IncTick();
}
