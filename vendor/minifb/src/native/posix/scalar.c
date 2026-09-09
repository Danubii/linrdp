#include <stdint.h>

static uint32_t bilinear_pixel(
    const uint32_t* src,
    const uint32_t stride,
    const uint32_t x,
    const uint32_t y,
    const uint32_t next_x,
    const uint32_t next_y,
    const uint32_t fx,
    const uint32_t fy
) {
    const uint32_t p00 = src[y * stride + x];
    const uint32_t p10 = src[y * stride + next_x];
    const uint32_t p01 = src[next_y * stride + x];
    const uint32_t p11 = src[next_y * stride + next_x];
    const uint32_t wx0 = 256 - fx;
    const uint32_t wy0 = 256 - fy;
    const uint32_t w00 = wx0 * wy0;
    const uint32_t w10 = fx * wy0;
    const uint32_t w01 = wx0 * fy;
    const uint32_t w11 = fx * fy;
    uint32_t out = 0;
    for (uint32_t shift = 0; shift < 32; shift += 8) {
        const uint32_t value =
            (((p00 >> shift) & 255) * w00 +
             ((p10 >> shift) & 255) * w10 +
             ((p01 >> shift) & 255) * w01 +
             ((p11 >> shift) & 255) * w11 + 32768) >> 16;
        out |= value << shift;
    }
    return out;
}

void image_resize_linear(
    uint32_t* dst,
    const uint32_t dst_width,
    const uint32_t dst_height,
    const uint32_t* src,
    const uint32_t src_width,
    const uint32_t src_height,
    const uint32_t src_stride
) {
    const uint32_t step_x = dst_width > 1 ? ((src_width - 1) << 16) / (dst_width - 1) : 0;
    const uint32_t step_y = dst_height > 1 ? ((src_height - 1) << 16) / (dst_height - 1) : 0;
    uint32_t fixed_y = 0;

    for (uint32_t i = 0; i < dst_height; i++) {
        const uint32_t y = fixed_y >> 16;
        const uint32_t next_y = y + (y + 1 < src_height);
        const uint32_t fy = (fixed_y >> 8) & 255;
        uint32_t fixed_x = 0;
        for (uint32_t j = 0; j < dst_width; j++) {
            const uint32_t x = fixed_x >> 16;
            const uint32_t next_x = x + (x + 1 < src_width);
            const uint32_t fx = (fixed_x >> 8) & 255;
            *dst++ = bilinear_pixel(src, src_stride, x, y, next_x, next_y, fx, fy);
            fixed_x += step_x;
        }
        fixed_y += step_y;
    }
}

static void image_resize_linear_stride(
    uint32_t* dst,
    const uint32_t dst_width,
    const uint32_t dst_height,
    const uint32_t* src,
    const uint32_t src_width,
    const uint32_t src_height,
    const uint32_t src_stride,
    const uint32_t stride
) {
    const uint32_t step_x = dst_width > 1 ? ((src_width - 1) << 16) / (dst_width - 1) : 0;
    const uint32_t step_y = dst_height > 1 ? ((src_height - 1) << 16) / (dst_height - 1) : 0;
    const int stride_step = stride - dst_width;
    uint32_t fixed_y = 0;

    for (uint32_t i = 0; i < dst_height; i++) {
        const uint32_t y = fixed_y >> 16;
        const uint32_t next_y = y + (y + 1 < src_height);
        const uint32_t fy = (fixed_y >> 8) & 255;
        uint32_t fixed_x = 0;
        for (uint32_t j = 0; j < dst_width; j++) {
            const uint32_t x = fixed_x >> 16;
            const uint32_t next_x = x + (x + 1 < src_width);
            const uint32_t fx = (fixed_x >> 8) & 255;
            *dst++ = bilinear_pixel(src, src_stride, x, y, next_x, next_y, fx, fy);
            fixed_x += step_x;
        }
        dst += stride_step;
        fixed_y += step_y;
    }
}

void image_resize_linear_aspect_fill(
    uint32_t* dst,
    const uint32_t dst_width,
    const uint32_t dst_height,
    const uint32_t* src,
    const uint32_t src_width,
    const uint32_t src_height,
    const uint32_t src_stride,
    const uint32_t bg_clear
) {
    // TODO: Optimize by only clearing the areas the image blit doesn't fill
    for (uint32_t i = 0; i < dst_width * dst_height; ++i) {
        dst[i] = bg_clear;
    }

    const float buffer_aspect = (float)(src_width) / (float)(src_height);
    const float win_aspect = (float)(dst_width) / (float)(dst_height);

    if (buffer_aspect > win_aspect) {
        const uint32_t new_height = (uint32_t)(dst_width / buffer_aspect);
        const int offset = (new_height - dst_height) / -2;
        image_resize_linear(
            dst + (offset * dst_width),
            dst_width, new_height,
            src, src_width, src_height, src_stride
        );
    } else {
        const uint32_t new_width = (uint32_t)(dst_height * buffer_aspect);
        const int offset = (new_width - dst_width) / -2;
        image_resize_linear_stride(
            dst + offset,
            new_width, dst_height,
            src, src_width, src_height, src_stride,
            dst_width
        );
    }
}

void image_center(
    uint32_t* dst,
    const uint32_t dst_width,
    const uint32_t dst_height,
    const uint32_t* src,
    const uint32_t src_width,
    const uint32_t src_height,
    const uint32_t src_stride,
    const uint32_t bg_clear
) {
    // TODO: Optimize by only clearing the areas the image blit doesn't fill
    for (uint32_t i = 0; i < dst_width * dst_height; ++i) {
        dst[i] = bg_clear;
    }

    if (src_height > dst_height) {
        const int y_offset = (src_height - dst_height) / 2;
        uint32_t new_height = src_height - y_offset;
        src += y_offset * src_stride;

        if (new_height > dst_height)
            new_height = dst_height;

        if (src_width > dst_width) {
            const int x_offset = (src_width - dst_width) / 2;
            src += x_offset;

            for (uint32_t y = 0; y < dst_height; ++y) {
                for (uint32_t x = 0; x < dst_width; ++x) {
                    *dst++ = *src++;
                }
                src += (src_stride - dst_width);
            }
        } else {
            const int x_offset = (dst_width - src_width) / 2;

            for (uint32_t y = 0; y < new_height; ++y) {
                dst += x_offset;

                for (uint32_t x = 0; x < src_width; ++x) {
                    *dst++ = *src++;
                }
                dst += (dst_width - (src_width + x_offset));
                src += src_stride - src_width;
            }
        }
    } else {
        const int y_offset = (dst_height - src_height) / 2;
        dst += y_offset * dst_width;

        if (src_width > dst_width) {
            const int x_offset = (src_width - dst_width) / 2;
            src += x_offset;

            for (uint32_t y = 0; y < src_height; ++y) {
                for (uint32_t x = 0; x < dst_width; ++x) {
                    *dst++ = *src++;
                }
                src += (src_stride - dst_width);
            }
        } else {
            const int x_offset = (dst_width - src_width) / 2;
            dst += x_offset;

            for (uint32_t y = 0; y < src_height; ++y) {
                for (uint32_t x = 0; x < src_width; ++x) {
                    *dst++ = *src++;
                }
                dst += (dst_width - src_width);
                src += src_stride - src_width;
            }
        }
    }
}

void image_upper_left(
    uint32_t* dst,
    const uint32_t dst_width,
    const uint32_t dst_height,
    const uint32_t* src,
    const uint32_t src_width,
    const uint32_t src_height,
    const uint32_t src_stride,
    const uint32_t bg_clear
) {
    // TODO: Optimize by only clearing the areas the image blit doesn't fill
    for (uint32_t i = 0; i < dst_width * dst_height; ++i) {
        dst[i] = bg_clear;
    }

    if (src_height > dst_height) {
        const int y_offset = (src_height - dst_height) / 2;
        const uint32_t new_height = src_height - y_offset;

        if (src_width > dst_width) {
            for (uint32_t y = 0; y < dst_height; ++y) {
                for (uint32_t x = 0; x < dst_width; ++x) {
                    *dst++ = *src++;
                }
                src += (src_stride - dst_width);
            }
        } else {
            for (uint32_t y = 0; y < new_height; ++y) {
                for (uint32_t x = 0; x < src_width; ++x) {
                    *dst++ = *src++;
                }
                dst += (dst_width - src_width);
                src += src_stride - src_width;
            }
        }
    } else {
        if (src_width > dst_width) {
            for (uint32_t y = 0; y < src_height; ++y) {
                for (uint32_t x = 0; x < dst_width; ++x) {
                    *dst++ = *src++;
                }
                src += (src_stride - dst_width);
            }
        } else {
            for (uint32_t y = 0; y < src_height; ++y) {
                for (uint32_t x = 0; x < src_width; ++x) {
                    *dst++ = *src++;
                }
                dst += (dst_width - src_width);
                src += src_stride - src_width;
            }
        }
    }
}
