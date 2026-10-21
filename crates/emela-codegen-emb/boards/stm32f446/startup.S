/* SPDX-License-Identifier: Apache-2.0 */
.syntax unified
.cpu cortex-m4
.fpu fpv4-sp-d16
.thumb

/* Weak handlers can be replaced by application C or assembly definitions. */
.macro vector name
    .word \name
    .weak \name
    .thumb_set \name, Default_Handler
.endm

.section .isr_vector,"a",%progbits
.global g_pfnVectors
.type g_pfnVectors, %object
g_pfnVectors:
    .word _estack
    .word Reset_Handler
    vector NMI_Handler
    vector HardFault_Handler
    vector MemManage_Handler
    vector BusFault_Handler
    vector UsageFault_Handler
    .word 0
    .word 0
    .word 0
    .word 0
    vector SVC_Handler
    vector DebugMon_Handler
    .word 0
    vector PendSV_Handler
    vector SysTick_Handler
    vector WWDG_IRQHandler
    vector PVD_IRQHandler
    vector TAMP_STAMP_IRQHandler
    vector RTC_WKUP_IRQHandler
    vector FLASH_IRQHandler
    vector RCC_IRQHandler
    vector EXTI0_IRQHandler
    vector EXTI1_IRQHandler
    vector EXTI2_IRQHandler
    vector EXTI3_IRQHandler
    vector EXTI4_IRQHandler
    vector DMA1_Stream0_IRQHandler
    vector DMA1_Stream1_IRQHandler
    vector DMA1_Stream2_IRQHandler
    vector DMA1_Stream3_IRQHandler
    vector DMA1_Stream4_IRQHandler
    vector DMA1_Stream5_IRQHandler
    vector DMA1_Stream6_IRQHandler
    vector ADC_IRQHandler
    vector CAN1_TX_IRQHandler
    vector CAN1_RX0_IRQHandler
    vector CAN1_RX1_IRQHandler
    vector CAN1_SCE_IRQHandler
    vector EXTI9_5_IRQHandler
    vector TIM1_BRK_TIM9_IRQHandler
    vector TIM1_UP_TIM10_IRQHandler
    vector TIM1_TRG_COM_TIM11_IRQHandler
    vector TIM1_CC_IRQHandler
    vector TIM2_IRQHandler
    vector TIM3_IRQHandler
    vector TIM4_IRQHandler
    vector I2C1_EV_IRQHandler
    vector I2C1_ER_IRQHandler
    vector I2C2_EV_IRQHandler
    vector I2C2_ER_IRQHandler
    vector SPI1_IRQHandler
    vector SPI2_IRQHandler
    vector USART1_IRQHandler
    vector USART2_IRQHandler
    vector USART3_IRQHandler
    vector EXTI15_10_IRQHandler
    vector RTC_Alarm_IRQHandler
    vector OTG_FS_WKUP_IRQHandler
    vector TIM8_BRK_TIM12_IRQHandler
    vector TIM8_UP_TIM13_IRQHandler
    vector TIM8_TRG_COM_TIM14_IRQHandler
    vector TIM8_CC_IRQHandler
    vector DMA1_Stream7_IRQHandler
    vector FMC_IRQHandler
    vector SDIO_IRQHandler
    vector TIM5_IRQHandler
    vector SPI3_IRQHandler
    vector UART4_IRQHandler
    vector UART5_IRQHandler
    vector TIM6_DAC_IRQHandler
    vector TIM7_IRQHandler
    vector DMA2_Stream0_IRQHandler
    vector DMA2_Stream1_IRQHandler
    vector DMA2_Stream2_IRQHandler
    vector DMA2_Stream3_IRQHandler
    vector DMA2_Stream4_IRQHandler
    .word 0
    .word 0
    vector CAN2_TX_IRQHandler
    vector CAN2_RX0_IRQHandler
    vector CAN2_RX1_IRQHandler
    vector CAN2_SCE_IRQHandler
    vector OTG_FS_IRQHandler
    vector DMA2_Stream5_IRQHandler
    vector DMA2_Stream6_IRQHandler
    vector DMA2_Stream7_IRQHandler
    vector USART6_IRQHandler
    vector I2C3_EV_IRQHandler
    vector I2C3_ER_IRQHandler
    vector OTG_HS_EP1_OUT_IRQHandler
    vector OTG_HS_EP1_IN_IRQHandler
    vector OTG_HS_WKUP_IRQHandler
    vector OTG_HS_IRQHandler
    vector DCMI_IRQHandler
    .word 0
    .word 0
    vector FPU_IRQHandler
    .word 0
    .word 0
    vector SPI4_IRQHandler
    .word 0
    .word 0
    vector SAI1_IRQHandler
    .word 0
    .word 0
    .word 0
    vector SAI2_IRQHandler
    vector QUADSPI_IRQHandler
    vector CEC_IRQHandler
    vector SPDIF_RX_IRQHandler
    vector FMPI2C1_EV_IRQHandler
    vector FMPI2C1_ER_IRQHandler
.size g_pfnVectors, .-g_pfnVectors

.section .text.Reset_Handler,"ax",%progbits
.global Reset_Handler
.type Reset_Handler, %function
.thumb_func
Reset_Handler:
    /* Enable the FPU before hard-float code; reset clock remains HSI at 16 MHz. */
    ldr r0, =0xe000ed88
    ldr r1, [r0]
    ldr r2, =0x00f00000
    orrs r1, r2
    str r1, [r0]
    ldr r0, =0xe000ed08
    ldr r1, =g_pfnVectors
    str r1, [r0]
    dsb
    isb
    ldr r0, =_sidata
    ldr r1, =_sdata
    ldr r2, =_edata
1:
    cmp r1, r2
    bhs 2f
    ldr r3, [r0], #4
    str r3, [r1], #4
    b 1b
2:
    ldr r1, =_sbss
    ldr r2, =_ebss
    movs r3, #0
3:
    cmp r1, r2
    bhs 4f
    str r3, [r1], #4
    b 3b
4:
    bl __libc_init_array
    bl main
5:
    wfi
    b 5b
.size Reset_Handler, .-Reset_Handler

.section .text.Default_Handler,"ax",%progbits
.global Default_Handler
.type Default_Handler, %function
.thumb_func
Default_Handler:
    cpsid i
6:
    wfi
    b 6b
.size Default_Handler, .-Default_Handler
.section .note.GNU-stack,"",%progbits
