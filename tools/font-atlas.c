/* Offline macOS atlas generator using CoreText, compiled with Apple Clang.
 * usage: font-atlas <font.ttf> <out.bin> [pixel-size cell-width cell-height baseline weight]
 * Defaults produce the 16 px interface atlas. */
#include <CoreText/CoreText.h>
#include <CoreGraphics/CoreGraphics.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>

int main(int argc, char **argv) {
    if (argc != 3 && argc != 8) return 1;
    double size = argc == 8 ? atof(argv[3]) : 16;
    int W = argc == 8 ? atoi(argv[4]) : 16;
    int H = argc == 8 ? atoi(argv[5]) : 22;
    double baseline = argc == 8 ? atof(argv[6]) : 5;
    double weight = argc == 8 ? atof(argv[7]) : 0;
    enum { COUNT = 191 };
    if (W < 2 || W > 255 || W % 2 || H < 2 || H > 255) return 1;
    CGDataProviderRef provider = CGDataProviderCreateWithFilename(argv[1]);
    if (!provider) return 2;
    CGFontRef graphics = CGFontCreateWithDataProvider(provider);
    if (!graphics) return 3;
    CTFontRef font = CTFontCreateWithGraphicsFont(graphics, size, NULL, NULL);
    if (weight > 0) {
        /* Variable-font weight axis ('wght'). */
        int tag = 0x77676874;
        CFNumberRef axis = CFNumberCreate(NULL, kCFNumberIntType, &tag);
        CFNumberRef value = CFNumberCreate(NULL, kCFNumberDoubleType, &weight);
        CFDictionaryRef variation = CFDictionaryCreate(NULL, (const void **)&axis, (const void **)&value, 1,
            &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
        CFStringRef key = kCTFontVariationAttribute;
        CFDictionaryRef attributes = CFDictionaryCreate(NULL, (const void **)&key, (const void **)&variation, 1,
            &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks);
        CTFontDescriptorRef descriptor = CTFontDescriptorCreateWithAttributes(attributes);
        CTFontRef varied = CTFontCreateCopyWithAttributes(font, size, NULL, descriptor);
        if (!varied) return 7;
        CFRelease(font);
        font = varied;
    }
    unsigned char *advances = calloc(COUNT, 1);
    unsigned char *atlas = calloc(COUNT * W * H / 2, 1);
    unsigned char *pixels = calloc(W * H, 1);
    if (!advances || !atlas || !pixels) return 8;
    for (int i = 0; i < COUNT; i++) {
        UniChar code = i < 95 ? i + 32 : 0x400 + i - 95;
        CGGlyph glyph;
        if (!CTFontGetGlyphsForCharacters(font, &code, &glyph, 1)) return 4;
        CGSize advance;
        CTFontGetAdvancesForGlyphs(font, kCTFontOrientationHorizontal, &glyph, &advance, 1);
        advances[i] = fmin(W, fmax(3, ceil(advance.width)));
        memset(pixels, 0, W * H);
        CGContextRef context = CGBitmapContextCreate(pixels, W, H, 8, W, NULL, kCGImageAlphaOnly);
        if (!context) return 5;
        CGContextSetShouldAntialias(context, true);
        CGContextSetGrayFillColor(context, 1, 1);
        CGPoint position = CGPointMake(0, baseline);
        CTFontDrawGlyphs(font, &glyph, &position, 1, context);
        CGContextRelease(context);
        for (int y = 0; y < H; y++) for (int x = 0; x < W; x += 2) {
            int a = (pixels[y*W+x]+8)/17;
            int b = (pixels[y*W+x+1]+8)/17;
            atlas[i*W*H/2+y*W/2+x/2] = (a << 4) | b;
        }
    }
    FILE *file = fopen(argv[2], "wb");
    if (!file) return 6;
    fwrite(advances, 1, COUNT, file);
    fwrite(atlas, 1, COUNT * W * H / 2, file);
    fclose(file);
    CFRelease(font); CGFontRelease(graphics); CGDataProviderRelease(provider);
    printf("Generated %d glyphs, %d bytes\n", COUNT, COUNT + COUNT * W * H / 2);
    return 0;
}
