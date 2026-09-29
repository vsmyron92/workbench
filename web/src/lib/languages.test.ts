import { describe, expect, it } from 'vitest'
import { languageFor } from './languages'

describe('languageFor', () => {
  it('maps C and C++ sources and headers', () => {
    expect(languageFor('src/main.c')).toBe('c')
    expect(languageFor('include/api.h')).toBe('cpp')
    for (const f of ['a.cc', 'a.cpp', 'a.CXX', 'a.c++', 'a.hpp', 'a.hh', 'a.hxx', 'a.h++', 'a.ipp', 'a.tpp', 'a.inl', 'a.ixx', 'a.cppm', 'sketch.ino', 'k.cu', 'k.cuh']) {
      expect(languageFor(f), f).toBe('cpp')
    }
  })

  it('maps Verilog, SystemVerilog and VHDL', () => {
    expect(languageFor('rtl/top.v')).toBe('verilog')
    expect(languageFor('rtl/defs.vh')).toBe('verilog')
    expect(languageFor('rtl/fifo.sv')).toBe('systemverilog')
    expect(languageFor('rtl/pkg.SVH')).toBe('systemverilog')
    expect(languageFor('rtl/alu.vhd')).toBe('vhdl')
    expect(languageFor('rtl/alu.vhdl')).toBe('vhdl')
    expect(languageFor('sim/alu_tb.vht')).toBe('vhdl')
  })

  it('keeps special names and unknown files', () => {
    expect(languageFor('Dockerfile.dev')).toBe('dockerfile')
    expect(languageFor('Makefile')).toBe('shell')
    expect(languageFor('notes.xyz')).toBe('plaintext')
    expect(languageFor('LICENSE')).toBe('plaintext')
  })
})
